//! 学校 SSO 的两枚 Cookie 与稳定设备标识；不保存网关候选或浏览器 profile。
//!
//! 新增档案由 `atrust_auth_storage` 沿现有配置目录和 key.bin 加密存取。
//! 此模块仅承载数据和 Cookie 事实，不证明认证成功，也不发网络请求。

use std::collections::BTreeMap;

use exv_vpn_atrust::Secret;
use zeroize::{Zeroize, Zeroizing};

pub const SSO_HOST: &str = "sso.ecnu.edu.cn";
const COOKIE_NAMES: [&str; 2] = ["SESSION", "SOURCEID_TGC"];
const WIRE_VERSION: u32 = 1;

/// 学校 Cookie 或档案格式的实际解析错误；不包含秘密值。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SsoJarError {
    Random,
    Cookie,
    Format,
    Version,
    DeviceId,
}

/// Cookie 值不实现 Clone；调试只报告存在性。
pub struct SsoSessionJar {
    cookies: BTreeMap<String, Secret>,
    device_id: Zeroizing<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
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

impl SsoSessionJar {
    /// 首次创建档案；后续采集和 CAS 轮换在同一档案上合并，保留 deviceId。
    ///
    /// # Errors
    /// 系统随机源不可用时返回 `Random`。
    pub fn new_fresh() -> Result<Self, SsoJarError> {
        let mut random = Zeroizing::new([0_u8; 32]);
        getrandom::fill(random.as_mut()).map_err(|_| SsoJarError::Random)?;
        let mut device_id = Zeroizing::new(String::with_capacity(64));
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for byte in random.iter() {
            device_id.push(char::from(HEX[usize::from(byte >> 4)]));
            device_id.push(char::from(HEX[usize::from(byte & 0xf)]));
        }
        Ok(Self {
            cookies: BTreeMap::new(),
            device_id,
        })
    }

    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    #[must_use]
    pub fn has_cookie(&self, name: &str) -> bool {
        self.cookies.contains_key(name)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cookies.is_empty()
    }

    #[must_use]
    pub fn cookie_names(&self) -> Vec<&str> {
        self.cookies.keys().map(String::as_str).collect()
    }

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

    /// 接收原生 Cookie 导出；仅保存当前学校的两枚 SSO Cookie，空值表示删除。
    ///
    /// # Errors
    /// Cookie 字符不适合 HTTP 请求头时返回 `Cookie`。
    pub fn insert_cookie(&mut self, name: &str, value: &str) -> Result<(), SsoJarError> {
        if !COOKIE_NAMES.contains(&name) {
            return Ok(());
        }
        if value.is_empty() {
            self.cookies.remove(name);
            return Ok(());
        }
        crate::atrust_cas_facts::apply_set_cookie_pair(&mut self.cookies, name, value)
            .map_err(|()| SsoJarError::Cookie)
    }

    /// CAS 响应 Cookie 的域、路径、过期删除规则与网关 Cookie 解析共用一份事实。
    pub fn merge_set_cookie(&mut self, header: &str) {
        let name = header
            .split(';')
            .next()
            .and_then(|pair| pair.trim().split_once('='))
            .map(|(name, _)| name);
        if name.is_some_and(|name| COOKIE_NAMES.contains(&name)) {
            crate::atrust_cas_facts::absorb_set_cookie(&mut self.cookies, header, SSO_HOST);
        }
    }

    pub(crate) fn encode(&self) -> Result<Zeroizing<Vec<u8>>, SsoJarError> {
        let wire = WireJar {
            version: WIRE_VERSION,
            device_id: self.device_id.to_string(),
            cookies: self
                .cookies
                .iter()
                .map(|(name, value)| (name.clone(), value.expose().to_owned()))
                .collect(),
        };
        serde_json::to_vec(&wire)
            .map(Zeroizing::new)
            .map_err(|_| SsoJarError::Format)
    }

    pub(crate) fn decode(bytes: &[u8]) -> Result<Self, SsoJarError> {
        let mut wire: WireJar = serde_json::from_slice(bytes).map_err(|_| SsoJarError::Format)?;
        if wire.version != WIRE_VERSION {
            return Err(SsoJarError::Version);
        }
        if wire.device_id.len() != 64
            || !wire
                .device_id
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(SsoJarError::DeviceId);
        }
        let mut jar = Self {
            cookies: BTreeMap::new(),
            device_id: Zeroizing::new(std::mem::take(&mut wire.device_id)),
        };
        for (name, value) in &mut wire.cookies {
            let result = jar.insert_cookie(name, value);
            value.zeroize();
            result?;
        }
        Ok(jar)
    }
}

impl std::fmt::Debug for SsoSessionJar {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SsoSessionJar")
            .field("cookie_names", &self.cookie_names())
            .field("device_id_present", &!self.device_id.is_empty())
            .finish()
    }
}
