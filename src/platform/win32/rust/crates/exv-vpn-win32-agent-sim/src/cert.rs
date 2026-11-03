//! 自建 CA + 叶子证书的生成、持久化与指纹。
//!
//! 方案（规格 §3）：自签 ECDSA P-256 根 CA（`basicConstraints=CA:TRUE`）+ 由它签发的
//! 叶子证书，SAN 含 `IP:127.0.0.1`、`DNS:localhost.sangfor.com.cn`、`DNS:localhost`。
//! 只为回环伪装 agent 服务；根 CA 经 [`crate::trust`] 装入**当前用户** Root 存储。
//!
//! 证书材料持久化在调用方给定的目录，便于跨次运行复用同一 CA（避免每次启动都往
//! 用户 Root 存储塞新根）与「一键卸载」时取回指纹。

use std::path::{Path, PathBuf};

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject;

/// 叶子证书 SAN（与门户硬编码候选主机逐字一致）。
pub const LEAF_SAN: [&str; 3] = ["127.0.0.1", "localhost.sangfor.com.cn", "localhost"];

const CA_PEM_FILE: &str = "ca-cert.pem";
const CA_KEY_FILE: &str = "ca-key.pem";
const LEAF_PEM_FILE: &str = "leaf-cert.pem";
const LEAF_KEY_FILE: &str = "leaf-key.pem";
const CA_SHA1_FILE: &str = "ca-sha1.txt";

/// 持久化的根 CA PEM 路径（`certutil -addstore` 的输入）。
#[must_use]
pub fn ca_pem_path(dir: &Path) -> PathBuf {
    dir.join(CA_PEM_FILE)
}

/// 持久化的叶子证书 PEM 路径（C4：共享 `exv-vpn-atrust` 本地 Agent 服务端的 TLS 材料）。
#[must_use]
pub fn leaf_pem_path(dir: &Path) -> PathBuf {
    dir.join(LEAF_PEM_FILE)
}

/// 持久化的叶子私钥 PEM 路径（与 [`leaf_pem_path`] 配对）。
#[must_use]
pub fn leaf_key_pem_path(dir: &Path) -> PathBuf {
    dir.join(LEAF_KEY_FILE)
}

/// 证书材料（DER + PEM + CA SHA-1 指纹）。
#[derive(Clone)]
pub struct CertMaterial {
    /// 根签名私钥，仅用于本地叶证书续签；旧材料没有此字段。
    pub ca_key_pem: Option<String>,
    /// 根 CA 证书 DER（TLS 链的信任锚）。
    pub ca_der: Vec<u8>,
    /// 根 CA 证书 PEM（`certutil -addstore` 的输入）。
    pub ca_pem: String,
    /// 叶子证书 DER。
    pub leaf_der: Vec<u8>,
    /// 叶子证书 PEM。
    pub leaf_pem: String,
    /// 叶子私钥 DER（PKCS#8）。
    pub leaf_key_der: Vec<u8>,
    /// 叶子私钥 PEM。
    pub leaf_key_pem: String,
    /// 根 CA 证书 SHA-1 指纹（大写十六进制，与 Windows 指纹/`certutil -delstore` 一致）。
    pub ca_sha1_hex: String,
}

/// 证书材料来源。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertOrigin {
    /// 复用已持久化的证书。
    Reused,
    /// 新生成；若替换了旧证书，携带旧 CA 的 SHA-1 指纹供调用方清理。
    Generated { replaced_sha1: Option<String> },
}

/// 证书错误。
#[derive(Debug)]
pub enum CertError {
    /// rcgen 生成/签名失败。
    Rcgen(String),
    /// 读写持久化文件失败。
    Io(String),
    /// 持久化文件损坏（PEM 不可解析）。
    Corrupt(String),
}

impl std::fmt::Display for CertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Rcgen(message) => write!(f, "证书生成失败: {message}"),
            Self::Io(message) => write!(f, "证书持久化失败: {message}"),
            Self::Corrupt(message) => write!(f, "证书文件损坏: {message}"),
        }
    }
}

impl std::error::Error for CertError {}

/// 读取已持久化的证书材料；文件不全或损坏时返回 `None`。
#[must_use]
pub fn load_material(dir: &Path) -> Option<CertMaterial> {
    let ca_pem = std::fs::read_to_string(dir.join(CA_PEM_FILE)).ok()?;
    let leaf_pem = std::fs::read_to_string(dir.join(LEAF_PEM_FILE)).ok()?;
    let leaf_key_pem = std::fs::read_to_string(dir.join(LEAF_KEY_FILE)).ok()?;
    let ca_sha1_hex = std::fs::read_to_string(dir.join(CA_SHA1_FILE)).ok()?;
    let mut material =
        material_from_pem(&ca_pem, &leaf_pem, &leaf_key_pem, ca_sha1_hex.trim()).ok()?;
    material.ca_key_pem = std::fs::read_to_string(dir.join(CA_KEY_FILE)).ok();
    Some(material)
}

/// 读取已持久化的证书指纹（无持久化则 `None`）。
#[must_use]
pub fn persisted_sha1(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join(CA_SHA1_FILE))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// 确保证书材料存在：能复用则复用，否则生成并持久化。
///
/// # Errors
/// 生成或写盘失败时返回 [`CertError`]。
pub fn ensure_material(dir: &Path) -> Result<(CertMaterial, CertOrigin), CertError> {
    if let Some(material) = load_material(dir) {
        return Ok((material, CertOrigin::Reused));
    }
    let replaced_sha1 = persisted_sha1(dir);
    let material = generate_material()?;
    persist_material(dir, &material)?;
    Ok((material, CertOrigin::Generated { replaced_sha1 }))
}

/// 生成一份全新的 CA + 叶子证书（不落盘）。
///
/// # Errors
/// rcgen 生成/签名失败时返回 [`CertError`]。
pub fn generate_material() -> Result<CertMaterial, CertError> {
    let ca_key = KeyPair::generate().map_err(|e| CertError::Rcgen(e.to_string()))?;
    generate_material_with_ca_key(&ca_key)
}

fn generate_material_with_ca_key(ca_key: &KeyPair) -> Result<CertMaterial, CertError> {
    let mut ca_params = CertificateParams::new(Vec::<String>::new())
        .map_err(|e| CertError::Rcgen(e.to_string()))?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, "EXV aTrust agent sim root CA");
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    let ca_cert = ca_params
        .self_signed(&ca_key)
        .map_err(|e| CertError::Rcgen(e.to_string()))?;

    let leaf_key = KeyPair::generate().map_err(|e| CertError::Rcgen(e.to_string()))?;
    let mut leaf_params = CertificateParams::new(
        LEAF_SAN
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
    .map_err(|e| CertError::Rcgen(e.to_string()))?;
    leaf_params.distinguished_name = leaf_distinguished_name();
    leaf_params.is_ca = IsCa::NoCa;
    leaf_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .map_err(|e| CertError::Rcgen(e.to_string()))?;

    let ca_der = ca_cert.der().to_vec();
    Ok(CertMaterial {
        ca_key_pem: Some(ca_key.serialize_pem()),
        ca_sha1_hex: sha1_hex(&ca_der),
        ca_pem: ca_cert.pem(),
        ca_der,
        leaf_pem: leaf_cert.pem(),
        leaf_der: leaf_cert.der().to_vec(),
        leaf_key_pem: leaf_key.serialize_pem(),
        leaf_key_der: leaf_key.serialize_der(),
    })
}

fn persist_material(dir: &Path, material: &CertMaterial) -> Result<(), CertError> {
    std::fs::create_dir_all(dir).map_err(|e| CertError::Io(e.to_string()))?;
    let write = |name: &str, bytes: &[u8]| -> Result<(), CertError> {
        std::fs::write(dir.join(name), bytes).map_err(|e| CertError::Io(e.to_string()))
    };
    if let Some(key) = &material.ca_key_pem {
        write(CA_KEY_FILE, key.as_bytes())?;
    }
    write(CA_PEM_FILE, material.ca_pem.as_bytes())?;
    write(LEAF_PEM_FILE, material.leaf_pem.as_bytes())?;
    write(LEAF_KEY_FILE, material.leaf_key_pem.as_bytes())?;
    write(CA_SHA1_FILE, material.ca_sha1_hex.as_bytes())
}

/// 叶子证书主题 DN（S4 对齐）。
///
/// 依据与边界：本机官方根 CA 主题实测为 `OU=Sangfor Technologies Inc., O=Sangfor
/// Technologies Inc., L=shenzhen, S=guangdong, C=CN`；官方**叶子**的精确 DN 无法从
/// 二进制确证（官方 agent 在 `httpServerWithSelfSignCaCert` 与
/// `httpServerWithCaOrganizationCert` 两路间选择，无静态叶子 DN）。故做**最小安全对齐**：
/// 在既有 `CN=localhost.sangfor.com.cn` 上补 `O`/`OU=Sangfor Technologies Inc.`，SAN
/// 不动、自建 CA 身份不动（不伪造官方根 CA）。
///
/// 迁移说明：该变更只影响**新生成**的材料；[`ensure_material`] 对已持久化材料原样复用
/// （同一自建 CA 继续受信，叶子 DN 维持旧值），不触发 CA 轮换、不在用户 Root 产生孤立
/// 根，也不需要用户再点一次根证书确认框。
fn leaf_distinguished_name() -> rcgen::DistinguishedName {
    let mut dn = rcgen::DistinguishedName::new();
    dn.push(DnType::CommonName, "localhost.sangfor.com.cn");
    dn.push(DnType::OrganizationalUnitName, "Sangfor Technologies Inc.");
    dn.push(DnType::OrganizationName, "Sangfor Technologies Inc.");
    dn
}

fn material_from_pem(
    ca_pem: &str,
    leaf_pem: &str,
    leaf_key_pem: &str,
    ca_sha1_hex: &str,
) -> Result<CertMaterial, CertError> {
    let ca = CertificateDer::from_pem_slice(ca_pem.as_bytes())
        .map_err(|e| CertError::Corrupt(e.to_string()))?;
    let leaf = CertificateDer::from_pem_slice(leaf_pem.as_bytes())
        .map_err(|e| CertError::Corrupt(e.to_string()))?;
    let key = rustls::pki_types::PrivateKeyDer::from_pem_slice(leaf_key_pem.as_bytes())
        .map_err(|e| CertError::Corrupt(e.to_string()))?;
    let leaf_key_der = key.secret_der().to_vec();
    Ok(CertMaterial {
        ca_key_pem: None,
        ca_der: ca.to_vec(),
        ca_pem: ca_pem.to_owned(),
        leaf_der: leaf.to_vec(),
        leaf_pem: leaf_pem.to_owned(),
        leaf_key_der,
        leaf_key_pem: leaf_key_pem.to_owned(),
        ca_sha1_hex: ca_sha1_hex.to_uppercase(),
    })
}

/// CA 证书 SHA-1 指纹（大写十六进制）。
#[must_use]
pub fn sha1_hex(der: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY, der);
    let mut out = String::with_capacity(digest.as_ref().len() * 2);
    for byte in digest.as_ref() {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

/// 证书目录（产品状态目录下的 `agent-sim` 子目录）。
#[must_use]
pub fn default_cert_dir() -> Option<PathBuf> {
    cert_dir_from_environment(
        std::env::var_os("LOCALAPPDATA"),
        std::env::var_os("USERPROFILE"),
    )
}

fn cert_dir_from_environment(
    local: Option<std::ffi::OsString>,
    profile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let local = local
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            profile
                .filter(|value| !value.is_empty())
                .map(|profile| PathBuf::from(profile).join("AppData").join("Local"))
        })?;
    Some(
        local
            .join("EXV")
            .join("profile")
            .join("default")
            .join("agent-sim"),
    )
}

pub fn material_usable_at(material: &CertMaterial, now: rustls::pki_types::UnixTime) -> bool {
    use rustls::client::danger::ServerCertVerifier as _;

    let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    if roots
        .add(CertificateDer::from(material.ca_der.clone()))
        .is_err()
    {
        return false;
    }
    let Ok(verifier) = rustls::client::WebPkiServerVerifier::builder_with_provider(
        std::sync::Arc::new(roots),
        std::sync::Arc::clone(&provider),
    )
    .build() else {
        return false;
    };
    let end_entity = CertificateDer::from(material.leaf_der.clone());
    for name in LEAF_SAN {
        let Ok(server_name) = rustls::pki_types::ServerName::try_from(name.to_owned()) else {
            return false;
        };
        if verifier
            .verify_server_cert(&end_entity, &[], &server_name, &[], now)
            .is_err()
        {
            return false;
        }
    }
    let Ok(key) =
        rustls::pki_types::PrivateKeyDer::from_pem_slice(material.leaf_key_pem.as_bytes())
    else {
        return false;
    };
    rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("TLS versions")
        .with_no_client_auth()
        .with_single_cert(vec![end_entity], key)
        .is_ok()
}

/// 本次系统浏览器认证的证书准备状态。检查阶段只读。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertificatePreparation {
    /// 已有用户根信任可直接复用，叶证书可按需本地续签。
    Ready,
    /// 安装或替换根信任之前，需要本次连接确认。
    ApprovalRequired { reason: String },
}

/// 证书检查和准备错误，保留底层信任错误。
#[derive(Debug)]
pub enum CertificateError {
    Material(CertError),
    Trust(crate::trust::TrustError),
    ApprovalRequired { reason: String },
}
impl std::fmt::Display for CertificateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Material(error) => error.fmt(f),
            Self::Trust(error) => error.fmt(f),
            Self::ApprovalRequired { reason } => write!(f, "{reason}"),
        }
    }
}
impl std::error::Error for CertificateError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Material(error) => Some(error),
            Self::Trust(error) => Some(error),
            _ => None,
        }
    }
}

/// 已就绪的本次 Agent TLS 文件，沿用原证书目录。
#[derive(Debug, Clone)]
pub struct PreparedAgentTls {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
}

fn existing_root(dir: &Path) -> Option<(String, Vec<u8>)> {
    let pem = std::fs::read_to_string(ca_pem_path(dir)).ok()?;
    let der = CertificateDer::from_pem_slice(pem.as_bytes())
        .ok()?
        .to_vec();
    Some((pem, der))
}

fn root_signing_key(dir: &Path, ca_der: &[u8]) -> Option<KeyPair> {
    let pem = std::fs::read_to_string(dir.join(CA_KEY_FILE)).ok()?;
    let key = KeyPair::from_pem(&pem).ok()?;
    let public_key = key.public_key_der();
    ca_der
        .windows(public_key.len())
        .any(|part| part == public_key)
        .then_some(key)
}

/// 只读检查证书与当前用户根信任。此函数不会生成或写入材料。
pub fn inspect_agent_certificate_with_store(
    dir: &Path,
    store: &dyn crate::trust::TrustStore,
) -> Result<CertificatePreparation, CertificateError> {
    if let Some((_, root)) = existing_root(dir) {
        let usable = load_material(dir).is_some_and(|material| {
            material_usable_at(&material, rustls::pki_types::UnixTime::now())
        });
        if store
            .is_trusted(&sha1_hex(&root))
            .map_err(CertificateError::Trust)?
            && (usable || root_signing_key(dir, &root).is_some())
        {
            return Ok(CertificatePreparation::Ready);
        }
    }
    Ok(CertificatePreparation::ApprovalRequired {
        reason: "系统浏览器认证需要安装 EXV 本地登录证书。继续后，系统将请求把证书加入当前用户的信任存储。".into(),
    })
}

/// 确认后准备材料。已有信任的根仅续签叶证书，不重复安装信任。
pub fn prepare_agent_certificate_with_store(
    dir: &Path,
    store: &dyn crate::trust::TrustStore,
    approved: bool,
) -> Result<PreparedAgentTls, CertificateError> {
    let state = inspect_agent_certificate_with_store(dir, store)?;
    if let CertificatePreparation::ApprovalRequired { reason } = &state {
        if !approved {
            return Err(CertificateError::ApprovalRequired {
                reason: reason.clone(),
            });
        }
    }
    let (material, needs_persist) = match load_material(dir)
        .filter(|material| material_usable_at(material, rustls::pki_types::UnixTime::now()))
    {
        Some(material) => (material, false),
        None => {
            let renewed = existing_root(dir).and_then(|(pem, der)| {
                let key = root_signing_key(dir, &der)?;
                Some((pem, der, key))
            });
            let material = if let Some((pem, der, key)) = renewed {
                let mut material =
                    generate_material_with_ca_key(&key).map_err(CertificateError::Material)?;
                material.ca_pem = pem;
                material.ca_sha1_hex = sha1_hex(&der);
                material.ca_der = der;
                if !material_usable_at(&material, rustls::pki_types::UnixTime::now()) {
                    return Err(CertificateError::Material(CertError::Corrupt(
                        "原根证书验签失败".into(),
                    )));
                }
                material
            } else {
                generate_material().map_err(CertificateError::Material)?
            };
            (material, true)
        }
    };
    if !store
        .is_trusted(&material.ca_sha1_hex)
        .map_err(CertificateError::Trust)?
    {
        // 信任可能在检查后被外部移除：仍需本次显式确认。
        if !approved {
            return Err(CertificateError::ApprovalRequired {
                reason: "系统浏览器认证需要安装 EXV 本地登录证书。".into(),
            });
        }
        if needs_persist {
            install_pending_root(dir, &material, store)?;
        } else {
            store
                .install(&ca_pem_path(dir))
                .map_err(CertificateError::Trust)?;
        }
    }
    if needs_persist {
        persist_material(dir, &material).map_err(CertificateError::Material)?;
    }
    Ok(PreparedAgentTls {
        cert_path: leaf_pem_path(dir),
        key_path: leaf_key_pem_path(dir),
    })
}

// 安装仅消费公开根 PEM；新叶和私钥留在内存，成功后再发布到固定路径。
fn install_pending_root(
    dir: &Path,
    material: &CertMaterial,
    store: &dyn crate::trust::TrustStore,
) -> Result<(), CertificateError> {
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = dir.join(format!(".pending-ca-{}-{sequence}.pem", std::process::id()));
    std::fs::create_dir_all(dir)
        .and_then(|()| std::fs::write(&path, material.ca_pem.as_bytes()))
        .map_err(|error| CertificateError::Material(CertError::Io(error.to_string())))?;
    let result = store.install(&path);
    let _ = std::fs::remove_file(&path);
    result.map_err(CertificateError::Trust)
}

