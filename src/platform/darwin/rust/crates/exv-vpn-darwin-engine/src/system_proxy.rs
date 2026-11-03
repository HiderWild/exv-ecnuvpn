//! 连接期系统代理例外与本机 PAC；沿用同一清理记录和资源生命周期。

use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

#[cfg(target_os = "macos")]
mod native;
mod pac;
#[cfg(target_os = "macos")]
pub use native::NativeStore;

pub const OWNER_PATH: &str =
    "/Library/Application Support/EXV/ServiceAgent/proxy-policy-owner.json";
pub type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PacSettings {
    pub enabled: Option<i32>,
    pub url: Option<String>,
}
impl PacSettings {
    fn enabled(&self) -> bool {
        self.enabled.unwrap_or(0) != 0
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    Exceptions(Option<Vec<String>>),
    Pac(PacSettings),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub exceptions: Option<Vec<String>>,
    pub manual_http: bool,
    pub automatic: bool,
    pub pac: PacSettings,
}

/// 每次事务在宿主配置锁内调用；实现必须保留其它 Proxies 字段。
pub trait Store {
    fn primary_service(&mut self) -> Result<String>;
    fn transaction<T>(
        &mut self,
        service: &str,
        action: impl FnOnce(Settings) -> Result<(T, Option<Update>)>,
    ) -> Result<T>;
    fn effective_exceptions(&mut self) -> Result<Option<Vec<String>>>;
    fn effective_pac(&mut self) -> Result<PacSettings>;
}

#[derive(Debug, Serialize, Deserialize)]
struct Record {
    service: String,
    original: Option<Vec<String>>,
    applied: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pac: Option<PacRecord>,
}
#[derive(Debug, Serialize, Deserialize)]
struct PacRecord {
    original: PacSettings,
    applied: PacSettings,
}

/// 同一 inode 的排他锁覆盖记录与网络字段整个生命周期。
pub struct Owner {
    file: File,
    endpoint: Option<pac::Endpoint>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApplyStatus {
    Applied,
    PacApplied,
    ChangedByOther,
    Unchanged,
    Disabled,
    AutomaticUnsupported,
}
#[derive(Debug, PartialEq, Eq)]
pub enum CleanupStatus {
    Empty,
    Restored,
    ChangedByOther,
}

impl Owner {
    pub fn effective_mode(&mut self, store: &mut impl Store) -> Result<Option<&'static str>> {
        let Some(record) = self.read()? else {
            return Ok(None);
        };
        if store.primary_service()? != record.service {
            return Ok(None);
        }
        let current = store.transaction(&record.service, |settings| Ok((settings, None)))?;
        if let Some(pac) = record.pac {
            if current.pac != pac.applied
                || !current.pac.enabled()
                || !self.endpoint.as_ref().is_some_and(pac::Endpoint::alive)
                || store.effective_pac()? != pac.applied
            {
                return Ok(None);
            }
            return Ok(Some(if current.manual_http {
                "mixed"
            } else {
                "automatic"
            }));
        }
        if current.manual_http
            && !current.automatic
            && current.exceptions == record.applied
            && store.effective_exceptions()? == record.applied
        {
            Ok(Some("manual"))
        } else {
            Ok(None)
        }
    }

    pub fn open(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.read(true).write(true).create(true).mode(0o600);
        let file = options
            .open(path)
            .map_err(|e| format!("proxy owner open: {e}"))?;
        // SAFETY: file 持有有效 fd；非阻塞排他锁避免清理活跃连接。
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(format!(
                "proxy owner busy: {}",
                std::io::Error::last_os_error()
            ));
        }
        // 首次创建的目录项也必须先于系统写入持久化。
        File::open(path.parent().ok_or("proxy owner parent missing")?)
            .and_then(|directory| directory.sync_all())
            .map_err(|e| format!("proxy owner directory sync: {e}"))?;
        Ok(Self {
            file,
            endpoint: None,
        })
    }

    fn read(&mut self) -> Result<Option<Record>> {
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        self.file
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        if bytes.is_empty() {
            return Ok(None);
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("proxy owner read: {e}"))
    }

    fn save(&mut self, record: Option<&Record>) -> Result<()> {
        let bytes = record
            .map(serde_json::to_vec)
            .transpose()
            .map_err(|e| e.to_string())?
            .unwrap_or_default();
        self.file
            .seek(SeekFrom::Start(0))
            .map_err(|e| e.to_string())?;
        self.file.write_all(&bytes).map_err(|e| e.to_string())?;
        self.file
            .set_len(bytes.len() as u64)
            .map_err(|e| e.to_string())?;
        self.file
            .sync_all()
            .map_err(|e| format!("proxy owner sync: {e}"))
    }

    pub fn cleanup(&mut self, store: &mut impl Store) -> Result<CleanupStatus> {
        let Some(record) = self.read()? else {
            self.close_endpoint()?;
            return Ok(CleanupStatus::Empty);
        };
        let result = store.transaction(&record.service, |current| {
            if let Some(pac) = &record.pac {
                if current.pac.url == pac.applied.url || current.pac.url == pac.original.url {
                    // URL 归本次所有；用户只切换开关时保留新开关，仍须移走本次端点 URL。
                    let restored = PacSettings {
                        url: pac.original.url.clone(),
                        ..current.pac
                    };
                    return Ok((CleanupStatus::Restored, Some(Update::Pac(restored))));
                }
                return Ok((CleanupStatus::ChangedByOther, None));
            }
            // Commit 已恢复但 Apply 失败时，Preferences 已为 original；仍须重试应用。
            if current.exceptions == record.applied || current.exceptions == record.original {
                Ok((
                    CleanupStatus::Restored,
                    Some(Update::Exceptions(record.original.clone())),
                ))
            } else {
                Ok((CleanupStatus::ChangedByOther, None))
            }
        })?;
        self.finish_cleanup(Self::close_endpoint)?;
        Ok(result)
    }

    fn finish_cleanup(&mut self, close: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        close(self)?;
        self.save(None)
    }

    fn close_endpoint(&mut self) -> Result<()> {
        if let Some(endpoint) = self.endpoint.as_mut() {
            endpoint.close()?;
        }
        self.endpoint = None;
        Ok(())
    }
    pub fn apply(&mut self, store: &mut impl Store, additions: &[String]) -> Result<ApplyStatus> {
        // 旧义务未完成不得覆盖；清理失败会保留记录并直接返回。
        self.cleanup(store)?;
        let service = store.primary_service()?;
        let observed = store.transaction(&service, |current| Ok((current, None)))?;
        if observed.automatic {
            if !observed.pac.enabled() {
                return Ok(ApplyStatus::AutomaticUnsupported);
            }
            let Some(url) = observed.pac.url.as_deref() else {
                return Ok(ApplyStatus::AutomaticUnsupported);
            };
            let script = pac::wrap(&pac::fetch(url)?, additions)?;
            let endpoint = pac::Endpoint::start(script)?;
            let applied = PacSettings {
                url: Some(endpoint.url().to_owned()),
                ..observed.pac.clone()
            };
            self.endpoint = Some(endpoint);
            let status = store.transaction(&service, |current| {
                if current.pac != observed.pac {
                    return Ok((ApplyStatus::ChangedByOther, None));
                }
                self.save(Some(&Record {
                    service: service.clone(),
                    original: None,
                    applied: None,
                    pac: Some(PacRecord {
                        original: observed.pac,
                        applied: applied.clone(),
                    }),
                }))?;
                Ok((ApplyStatus::PacApplied, Some(Update::Pac(applied.clone()))))
            })?;
            if status != ApplyStatus::PacApplied {
                self.close_endpoint()?;
            }
            return Ok(status);
        }
        let status = store.transaction(&service, |current| {
            if current.automatic {
                return Ok((ApplyStatus::AutomaticUnsupported, None));
            }
            if !current.manual_http {
                return Ok((ApplyStatus::Disabled, None));
            }
            let mut merged = current.exceptions.clone().unwrap_or_default();
            for item in additions {
                if !merged.contains(item) {
                    merged.push(item.clone());
                }
            }
            let applied = Some(merged);
            if applied == current.exceptions {
                return Ok((ApplyStatus::Unchanged, None));
            }
            self.save(Some(&Record {
                service: service.clone(),
                original: current.exceptions,
                applied: applied.clone(),
                pac: None,
            }))?;
            Ok((ApplyStatus::Applied, Some(Update::Exceptions(applied))))
        })?;
        Ok(status)
    }
}

/// 正式 Engine 启动与 ServiceAgent 内部固定维护入口共用。
#[cfg(target_os = "macos")]
pub fn cleanup_installed() -> Result<CleanupStatus> {
    let path = Path::new(OWNER_PATH);
    if !path.try_exists().map_err(|e| e.to_string())? {
        return Ok(CleanupStatus::Empty);
    }
    Owner::open(path)?.cleanup(&mut NativeStore)
}

/// 仅从实际承载的校园 IPv4 路由与既有 resolver 域构造例外。
pub fn additions(routes: &[String], domains: &[String]) -> Result<Vec<String>> {
    let mut result = Vec::new();
    for route in routes {
        let (address, prefix) = route.split_once('/').ok_or("proxy route missing prefix")?;
        let address: std::net::Ipv4Addr = address.parse().map_err(|_| "proxy invalid IPv4")?;
        let prefix: u32 = prefix.parse().map_err(|_| "proxy invalid prefix")?;
        if prefix > 32 {
            return Err("proxy prefix exceeds 32".into());
        }
        let mask = if prefix == 0 {
            0
        } else {
            u32::MAX << (32 - prefix)
        };
        let value = format!(
            "{}/{prefix}",
            std::net::Ipv4Addr::from(u32::from(address) & mask)
        );
        if !result.contains(&value) {
            result.push(value);
        }
    }
    for domain in domains {
        for value in [domain.clone(), format!("*.{domain}")] {
            if !result.contains(&value) {
                result.push(value);
            }
        }
    }
    Ok(result)
}

