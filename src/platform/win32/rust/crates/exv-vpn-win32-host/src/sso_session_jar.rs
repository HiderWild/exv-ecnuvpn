
use exv_vpn_atrust::Secret;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, Zeroizing};

/// SSO 档案绑定的学校统一认证 origin（与 `school_auto_login::SCHOOL_LOGIN_ORIGIN` 同一事实，
/// 这里只取 host 部分做域匹配与落盘命名）。
pub const SSO_HOST: &str = "sso.ecnu.edu.cn";

/// 档案 wire 版本；将来字段演进时递增并保留向后解析。
const WIRE_VERSION: u32 = 1;

fn jar_path(profile: &Path) -> PathBuf {
    use std::fmt::Write as _;
    let mut digest = String::with_capacity(64);
    for byte in Sha256::digest(SSO_HOST.as_bytes()) {
        // 摘要字节恒可写，忽略 write! 的不可失败错误。
        let _ = write!(digest, "{byte:02x}");
    }
    profile.join(format!("sso-{digest}.dpapi"))
}

/// 生成稳定 deviceId 的随机源形态：32 字节 → 64 位小写十六进制。
fn random_device_id() -> Result<Zeroizing<String>, String> {
    let mut bytes = [0u8; 32];
    ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut bytes)
        .map_err(|_| "无法生成无头设备标识".to_owned())?;
    let mut hex = Zeroizing::new(String::with_capacity(64));
    for byte in bytes {
        hex.push(char::from(b"0123456789abcdef"[usize::from(byte >> 4)]));
        hex.push(char::from(b"0123456789abcdef"[usize::from(byte & 0x0f)]));
    }
    bytes.zeroize();
    Ok(hex)
}

/// 档案的落盘 wire 形态（DPAPI 加密前的 JSON）；字符串字段 drop 时零化。
#[derive(serde::Deserialize, serde::Serialize)]
struct WireJar {
    version: u32,
    #[serde(rename = "deviceId")]
    device_id: String,
    cookies: BTreeMap<String, String>,
}

impl Drop for WireJar {
    fn drop(&mut self) {
        self.device_id.zeroize();
        for value in self.cookies.values_mut() {
            value.zeroize();
        }
    }
}

/// 学校 SSO 域 cookie 档案 + 稳定 `deviceId`。
///
/// cookie 名固定保留 `SESSION`/`SOURCEID_TGC` 等本域键（不白名单过滤：SSO 域 benign
/// 键一并保留，反域/跨域由 [`Self::merge_set_cookie`] 拒绝）。值全部经 [`Secret`] 持有。
pub struct SsoSessionJar {
    cookies: BTreeMap<String, Secret>,
    device_id: Zeroizing<String>,
}

impl SsoSessionJar {
    /// 首次使用：生成新 `deviceId`（64-hex），cookie 集为空；由调用方在链路结束后
    /// [`save`] 落盘以保持「首次生成、永久复用」。
    /// # Errors
    /// 系统随机源不可用时返回错误字符串（不伪造设备标识）。
    pub fn new_fresh() -> Result<Self, String> {
        Ok(Self {
            cookies: BTreeMap::new(),
            device_id: random_device_id()?,
        })
    }

    /// 稳定设备标识（64 位小写十六进制）。不落日志；上送网关时作为协议字段使用。
    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// 渲染为对 SSO origin 的 Cookie 请求头（`SESSION=…; SOURCEID_TGC=…`，`BTreeMap` 序）。
    /// 结果由零化容器持有，调用方负责不落日志。
    #[must_use]
    pub fn cookie_header(&self) -> Zeroizing<String> {
        let mut header = Zeroizing::new(String::new());
        for (name, value) in &self.cookies {
            if !header.is_empty() {
                header.push_str("; ");
            }
            header.push_str(name);
            header.push('=');
            header.push_str(value.expose());
        }
        header
    }

    /// cookie 名集合（有序）；仅供日志/调试输出，不含值。
    #[must_use]
    pub fn cookie_names(&self) -> Vec<&str> {
        self.cookies.keys().map(String::as_str).collect()
    }

    /// 是否持有指定 cookie（存在性判断，不取值）。
    #[must_use]
    pub fn has_cookie(&self, name: &str) -> bool {
        self.cookies.contains_key(name)
    }

    /// 直接注入一枚 cookie（阶段 B 窗口导出入口用）；同名覆盖。
    /// # Errors
    /// cookie 名/值字符非法时返回错误（与请求头回放约束一致）。
    pub fn insert_cookie(&mut self, name: &str, value: &str) -> Result<(), String> {
        crate::atrust_cas_login::apply_set_cookie_pair(&mut self.cookies, name, value)
            .map_err(|()| "SSO cookie 名或值非法".to_owned())
    }

    /// 吸收一条针对 SSO origin 的 `Set-Cookie` 头：跨域（`Domain` 非本域）、非根路径、
    /// 过期删除（空值或 `Max-Age<=0`）按浏览器语义处理；非法字符静默忽略（cookie 是
    /// 附带状态，忽略不劣于没有；链路结果由上层分类兜底）。
    pub fn merge_set_cookie(&mut self, header_value: &str) {
        crate::atrust_cas_login::absorb_set_cookie(&mut self.cookies, header_value, SSO_HOST);
    }
}

// 只打印 cookie 名集合与 deviceId 存在性/长度；值是敏感材料，绝不进入 Debug。
impl std::fmt::Debug for SsoSessionJar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SsoSessionJar")
            .field("cookies", &self.cookie_names())
            .field("device_id_len", &self.device_id.len())
            .finish()
    }
}

/// 把档案加密写入 EXV 配置目录下的 `sso-<sha256(host)>.dpapi`。
/// # Errors
/// 编码、DPAPI 或文件系统失败返回错误字符串（不含 cookie/deviceId 值）。
pub fn save(profile: &Path, jar: &SsoSessionJar) -> Result<(), String> {
    let wire = WireJar {
        version: WIRE_VERSION,
        device_id: jar.device_id.to_string(),
        cookies: jar
            .cookies
            .iter()
            .map(|(name, value)| (name.clone(), value.expose().to_owned()))
            .collect(),
    };
    let bytes = Zeroizing::new(serde_json::to_vec(&wire).map_err(|_| "SSO 档案编码失败")?);
    let sealed = crate::atrust_session_cache::crypt(&bytes, true)?;
    std::fs::create_dir_all(profile).map_err(|_| "无法创建 SSO 档案目录")?;
    std::fs::write(jar_path(profile), &*sealed).map_err(|_| "无法保存 SSO 档案".to_owned())
}

/// 读取档案；无档案返回 `None`。
/// # Errors
/// 读取、DPAPI 解密或形状不符（含版本未知）返回错误字符串；调用方可选择丢弃重建。
pub fn load(profile: &Path) -> Result<Option<SsoSessionJar>, String> {
    let sealed = match std::fs::read(jar_path(profile)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取 SSO 档案".into()),
    };
    let bytes = crate::atrust_session_cache::crypt(&sealed, false)?;
    let mut wire: WireJar =
        serde_json::from_slice(&bytes).map_err(|_| "SSO 档案格式无效".to_owned())?;
    if wire.version != WIRE_VERSION {
        return Err("SSO 档案版本未知".to_owned());
    }
    if wire.device_id.len() != 64
        || !wire
            .device_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("SSO 档案设备标识无效".to_owned());
    }
    let mut cookies = BTreeMap::new();
    for (name, value) in std::mem::take(&mut wire.cookies) {
        cookies.insert(name, Secret::new(value));
    }
    let jar = SsoSessionJar {
        cookies,
        device_id: Zeroizing::new(std::mem::take(&mut wire.device_id)),
    };
    Ok(Some(jar))
}

/// 尽力删除档案（无头档案被判污染时的重置入口）；失败不阻塞重新登录。
pub fn forget(profile: &Path) {
    let _ = std::fs::remove_file(jar_path(profile));
}
