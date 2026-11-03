
use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use rcgen::{
    BasicConstraints, CertificateParams, DnType, ExtendedKeyUsagePurpose, IsCa, KeyPair,
    KeyUsagePurpose,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime, pem::PemObject};

use crate::config_paths;

/// 根 CA 主题 CN（同时是钥匙串 `find-certificate -c` 的匹配名与卸载清场依据）。
pub const CA_COMMON_NAME: &str = "EXV Local Agent CA";
/// 叶子证书 SAN：`IP:127.0.0.1`（门户探测硬编码目标）+ `DNS:localhost` 备用。
/// rcgen 对可解析为 IP 的字符串自动落成 iPAddress SAN，其余落 dNSName。
pub const LEAF_SAN: [&str; 3] = ["127.0.0.1", "localhost.sangfor.com.cn", "localhost"];

/// 根 CA 证书 PEM 文件名。
pub const CA_PEM_FILE: &str = "root_ca.pem";
/// 根签名私钥，按原秘密文件权限保存。
pub const CA_KEY_FILE: &str = "root_ca.key";
/// 叶子证书 PEM 文件名。
pub const LEAF_PEM_FILE: &str = "leaf.pem";
/// 叶子私钥 PEM 文件名（0600）。
pub const LEAF_KEY_FILE: &str = "leaf.key";

/// 根 CA 有效期（天）≈ 10 年。
const CA_VALIDITY_DAYS: i64 = 3650;
/// 叶子证书有效期（天）= 1 年。
const LEAF_VALIDITY_DAYS: i64 = 365;
/// 签发回拨（分钟）：吸收同机时钟微小偏差，避免「生成后立刻不可信」窗口。
const BACKDATE_MINUTES: i64 = 60;

/// 钥匙串导出临时文件序号（进程内唯一命名）。
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 证书材料（PEM 三件套 + DER/私钥 DER）。
#[derive(Clone)]
pub struct AgentCertMaterial {
    /// 根签名私钥；旧版本材料未保存根钥。
    pub ca_key_pem: Option<String>,
    /// 根 CA 证书 PEM（`security add-trusted-cert` 的输入）。
    pub ca_pem: String,
    /// 根 CA 证书 DER。
    pub ca_der: Vec<u8>,
    /// 叶子证书 PEM。
    pub leaf_pem: String,
    /// 叶子证书 DER。
    pub leaf_der: Vec<u8>,
    /// 叶子私钥 PEM（PKCS#8）。
    pub leaf_key_pem: String,
    /// 叶子私钥 DER（PKCS#8）。
    pub leaf_key_der: Vec<u8>,
}

/// 证书材料来源；重生成时携带旧根 CA PEM 供调用方清理钥匙串旧信任。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentCertOrigin {
    /// 复用已持久化且当前可用的证书。
    Reused,
    /// 新生成；若替换了旧材料，携带旧根 CA PEM（可能是过期对，也可能是损坏残留）。
    Generated {
        /// 被替换的旧根 CA PEM（文件缺失时为 `None`）。
        replaced_ca_pem: Option<String>,
    },
}

/// 证书管理错误。
#[derive(Debug)]
pub enum AgentCertError {
    /// rcgen 生成/签名失败。
    Rcgen(String),
    /// 本次连接需要先确认安装根信任。
    ApprovalRequired { reason: String },
    /// 读写持久化文件失败。
    Io(String),
    /// 持久化文件损坏（PEM 不可解析或钥匙与证书不配对）。
    Corrupt(String),
    /// `/usr/bin/security` 调用失败（无法启动/非零退出/输出不可解析）。
    Security(String),
}

impl std::fmt::Display for AgentCertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ApprovalRequired { reason } => write!(f, "{reason}"),
            Self::Rcgen(message) => write!(f, "证书生成失败: {message}"),
            Self::Io(message) => write!(f, "证书持久化失败: {message}"),
            Self::Corrupt(message) => write!(f, "证书文件损坏: {message}"),
            Self::Security(message) => write!(f, "钥匙串操作失败: {message}"),
        }
    }
}

impl std::error::Error for AgentCertError {}

/// 证书目录（`<home>/Library/Application Support/EXV/certs`）。
#[must_use]
pub fn cert_dir_from_home(home: &Path) -> PathBuf {
    home.join("Library")
        .join("Application Support")
        .join("EXV")
        .join("certs")
}

/// 默认证书目录：`EXV_AGENT_CERT_DIR` 覆盖优先（测试/隔离用），否则
/// `HOME` 下的 Application Support 路径；两者皆缺时 `None`。
#[must_use]
pub fn default_cert_dir() -> Option<PathBuf> {
    cert_dir_from_environment(
        std::env::var_os("EXV_AGENT_CERT_DIR"),
        std::env::var_os("HOME"),
    )
}

fn cert_dir_from_environment(
    override_dir: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = override_dir.filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    let home = home.filter(|value| !value.is_empty())?;
    Some(cert_dir_from_home(Path::new(&home)))
}

/// 持久化根 CA PEM 路径（`security add-trusted-cert` 的输入）。
#[must_use]
pub fn root_ca_pem_path(dir: &Path) -> PathBuf {
    dir.join(CA_PEM_FILE)
}

/// 持久化叶子证书 PEM 路径。
#[must_use]
pub fn leaf_cert_pem_path(dir: &Path) -> PathBuf {
    dir.join(LEAF_PEM_FILE)
}

/// 持久化叶子私钥 PEM 路径（0600）。
#[must_use]
pub fn leaf_key_pem_path(dir: &Path) -> PathBuf {
    dir.join(LEAF_KEY_FILE)
}

/// 生成一份全新的根 CA + 叶子证书（不落盘），有效期从当前时刻起算。
///
/// # Errors
/// rcgen 生成/签名失败时返回 [`AgentCertError::Rcgen`]。
pub fn generate_material() -> Result<AgentCertMaterial, AgentCertError> {
    generate_material_at(time::OffsetDateTime::now_utc())
}

fn generate_material_at(now: time::OffsetDateTime) -> Result<AgentCertMaterial, AgentCertError> {
    let ca_key = KeyPair::generate().map_err(|error| AgentCertError::Rcgen(error.to_string()))?;
    generate_material_with_ca_key_at(&ca_key, now)
}

fn generate_material_with_ca_key_at(
    ca_key: &KeyPair,
    now: time::OffsetDateTime,
) -> Result<AgentCertMaterial, AgentCertError> {
    let backdate = time::Duration::minutes(BACKDATE_MINUTES);
    let rcgen_error = |message: rcgen::Error| AgentCertError::Rcgen(message.to_string());
    let mut ca_params = CertificateParams::new(Vec::<String>::new()).map_err(rcgen_error)?;
    ca_params.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
    ca_params
        .distinguished_name
        .push(DnType::CommonName, CA_COMMON_NAME);
    ca_params.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    ca_params.not_before = now - backdate;
    ca_params.not_after = now + time::Duration::days(CA_VALIDITY_DAYS);
    let ca_cert = ca_params.self_signed(&ca_key).map_err(rcgen_error)?;

    let leaf_key = KeyPair::generate().map_err(rcgen_error)?;
    let mut leaf_params = CertificateParams::new(
        LEAF_SAN
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>(),
    )
    .map_err(rcgen_error)?;
    leaf_params
        .distinguished_name
        .push(DnType::CommonName, "localhost");
    leaf_params.is_ca = IsCa::NoCa;
    leaf_params.key_usages = vec![
        KeyUsagePurpose::DigitalSignature,
        KeyUsagePurpose::KeyEncipherment,
    ];
    leaf_params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    leaf_params.not_before = now - backdate;
    leaf_params.not_after = now + time::Duration::days(LEAF_VALIDITY_DAYS);
    let leaf_cert = leaf_params
        .signed_by(&leaf_key, &ca_cert, &ca_key)
        .map_err(rcgen_error)?;

    Ok(AgentCertMaterial {
        ca_key_pem: Some(ca_key.serialize_pem()),
        ca_der: ca_cert.der().to_vec(),
        ca_pem: ca_cert.pem(),
        leaf_der: leaf_cert.der().to_vec(),
        leaf_pem: leaf_cert.pem(),
        leaf_key_der: leaf_key.serialize_der(),
        leaf_key_pem: leaf_key.serialize_pem(),
    })
}

/// 读取已持久化的证书材料；文件不全或 PEM 不可解析时返回 `None`（是否「可用」
/// 另由 `material_usable_at` 判定——读取成功不等于未过期）。
#[must_use]
pub fn load_material(dir: &Path) -> Option<AgentCertMaterial> {
    let ca_pem = std::fs::read_to_string(root_ca_pem_path(dir)).ok()?;
    let leaf_pem = std::fs::read_to_string(leaf_cert_pem_path(dir)).ok()?;
    let leaf_key_pem = std::fs::read_to_string(leaf_key_pem_path(dir)).ok()?;
    let mut material = material_from_pem(&ca_pem, &leaf_pem, &leaf_key_pem).ok()?;
    material.ca_key_pem = std::fs::read_to_string(dir.join(CA_KEY_FILE)).ok();
    Some(material)
}

fn material_from_pem(
    ca_pem: &str,
    leaf_pem: &str,
    leaf_key_pem: &str,
) -> Result<AgentCertMaterial, AgentCertError> {
    let ca = CertificateDer::from_pem_slice(ca_pem.as_bytes())
        .map_err(|error| AgentCertError::Corrupt(format!("root_ca.pem: {error}")))?;
    let leaf = CertificateDer::from_pem_slice(leaf_pem.as_bytes())
        .map_err(|error| AgentCertError::Corrupt(format!("leaf.pem: {error}")))?;
    let key = PrivateKeyDer::from_pem_slice(leaf_key_pem.as_bytes())
        .map_err(|error| AgentCertError::Corrupt(format!("leaf.key: {error}")))?;
    Ok(AgentCertMaterial {
        ca_key_pem: None,
        ca_der: ca.to_vec(),
        ca_pem: ca_pem.to_owned(),
        leaf_der: leaf.to_vec(),
        leaf_pem: leaf_pem.to_owned(),
        leaf_key_der: key.secret_der().to_vec(),
        leaf_key_pem: leaf_key_pem.to_owned(),
    })
}

/// 判定材料在给定时刻是否可用：叶子证书在该时刻经自建 CA 验签通过（覆盖有效期
/// 与链关系）、SAN 同时覆盖 `IP:127.0.0.1` 与 `DNS:localhost`，且私钥与叶子证书
/// 配对。任一不满足即视为不可用（过期、损坏、不配对统一走重生成）。
#[must_use]
pub fn material_usable_at(material: &AgentCertMaterial, now: UnixTime) -> bool {
    use rustls::client::danger::ServerCertVerifier as _;

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore::empty();
    if roots
        .add(CertificateDer::from(material.ca_der.clone()))
        .is_err()
    {
        return false;
    }
    let Ok(verifier) = rustls::client::WebPkiServerVerifier::builder_with_provider(
        Arc::new(roots),
        Arc::clone(&provider),
    )
    .build() else {
        return false;
    };
    let end_entity = CertificateDer::from(material.leaf_der.clone());
    for name in LEAF_SAN {
        let Ok(server_name) = ServerName::try_from(name.to_owned()) else {
            return false;
        };
        if verifier
            .verify_server_cert(&end_entity, &[], &server_name, &[], now)
            .is_err()
        {
            return false;
        }
    }
    let Ok(key) = PrivateKeyDer::from_pem_slice(material.leaf_key_pem.as_bytes()) else {
        return false;
    };
    rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![end_entity], key)
        .is_ok()
}

/// 确保证书材料存在并可用：能复用则复用，过期/损坏/不配对则整对重生成并落盘。
///
/// # Errors
/// 生成或写盘失败时返回 [`AgentCertError`]。
pub fn ensure_material(dir: &Path) -> Result<(AgentCertMaterial, AgentCertOrigin), AgentCertError> {
    ensure_material_at(dir, UnixTime::now())
}

fn ensure_material_at(
    dir: &Path,
    now: UnixTime,
) -> Result<(AgentCertMaterial, AgentCertOrigin), AgentCertError> {
    if let Some(material) = load_material(dir) {
        if material_usable_at(&material, now) {
            return Ok((material, AgentCertOrigin::Reused));
        }
        // 过期/损坏/不配对：整对重生成（CA 私钥不落盘，无法只续叶子），
        // 旧根 CA PEM 交还调用方清理钥匙串旧信任。
        let replaced_ca_pem = Some(material.ca_pem);
        let fresh = generate_material()?;
        persist_material(dir, &fresh)?;
        return Ok((fresh, AgentCertOrigin::Generated { replaced_ca_pem }));
    }
    // 文件缺失/不可解析；root_ca.pem 若仍可读则同样交还调用方清理。
    let replaced_ca_pem = persisted_root_ca_pem(dir);
    let fresh = generate_material()?;
    persist_material(dir, &fresh)?;
    Ok((fresh, AgentCertOrigin::Generated { replaced_ca_pem }))
}

fn persisted_root_ca_pem(dir: &Path) -> Option<String> {
    std::fs::read_to_string(root_ca_pem_path(dir))
        .ok()
        .filter(|value| !value.trim().is_empty())
}

fn persist_material(dir: &Path, material: &AgentCertMaterial) -> Result<(), AgentCertError> {
    let io_error =
        |name: &str, error: std::io::Error| AgentCertError::Io(format!("{name}: {error}"));
    if let Some(key) = &material.ca_key_pem {
        config_paths::atomic_write(&dir.join(CA_KEY_FILE), key.as_bytes())
            .map_err(|error| io_error(CA_KEY_FILE, error))?;
    }
    config_paths::atomic_write(&root_ca_pem_path(dir), material.ca_pem.as_bytes())
        .map_err(|error| io_error(CA_PEM_FILE, error))?;
    config_paths::atomic_write(&leaf_cert_pem_path(dir), material.leaf_pem.as_bytes())
        .map_err(|error| io_error(LEAF_PEM_FILE, error))?;
    config_paths::atomic_write(&leaf_key_pem_path(dir), material.leaf_key_pem.as_bytes())
        .map_err(|error| io_error(LEAF_KEY_FILE, error))?;
    Ok(())
}

/// 删除证书目录下的三件持久化文件（EXV 卸载钩子用；信任清理另走
/// [`uninstall_agent_trust`]）。返回删除的文件数（`NotFound` 视为已不存在）。
///
/// # Errors
/// 除「文件不存在」外的删除失败时返回 [`AgentCertError::Io`]。
pub fn remove_cert_files(dir: &Path) -> Result<usize, AgentCertError> {
    let mut removed = 0;
    for path in [
        dir.join(CA_KEY_FILE),
        root_ca_pem_path(dir),
        leaf_cert_pem_path(dir),
        leaf_key_pem_path(dir),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(AgentCertError::Io(format!(
                    "删除 {}: {error}",
                    path.display()
                )));
            }
        }
    }
    Ok(removed)
}

/// 供 C1 本地 Agent 服务端使用的 TLS 配置材料（叶子证书 PEM + 私钥 PEM）。
#[derive(Clone)]
pub struct AgentServerConfig {
    /// 叶子证书 PEM（rustls 服务端链的唯一端实体——本地服务端不发中间链）。
    pub leaf_cert_pem: String,
    /// 叶子私钥 PEM（PKCS#8）。
    pub leaf_key_pem: String,
    /// 根 CA 证书 PEM（诊断/导出用途；服务端配置不消费它）。
    pub ca_cert_pem: String,
}

impl AgentServerConfig {
    /// 从证书材料提取服务端配置材料。
    #[must_use]
    pub fn from_material(material: &AgentCertMaterial) -> Self {
        Self {
            leaf_cert_pem: material.leaf_pem.clone(),
            leaf_key_pem: material.leaf_key_pem.clone(),
            ca_cert_pem: material.ca_pem.clone(),
        }
    }

    /// 构建 rustls 服务端配置（C1 `AgentServer` 的接线点）。
    ///
    /// # Errors
    /// PEM 不可解析或证书与私钥不配对时返回 [`AgentCertError::Corrupt`]。
    pub fn to_rustls_server_config(&self) -> Result<rustls::ServerConfig, AgentCertError> {
        let cert = CertificateDer::from_pem_slice(self.leaf_cert_pem.as_bytes())
            .map_err(|error| AgentCertError::Corrupt(format!("leaf_cert_pem: {error}")))?;
        let key = PrivateKeyDer::from_pem_slice(self.leaf_key_pem.as_bytes())
            .map_err(|error| AgentCertError::Corrupt(format!("leaf_key_pem: {error}")))?;
        rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(vec![cert], key)
            .map_err(|error| AgentCertError::Corrupt(format!("证书与私钥不配对: {error}")))
    }
}

/// 解析 `security default-keychain` 输出（形如带引号的单行绝对路径）。
/// 输出为空、非绝对路径或多行杂讯时返回 `None`。
#[must_use]
pub fn parse_login_keychain_output(output: &str) -> Option<PathBuf> {
    let first_line = output
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())?;
    let trimmed = first_line.trim_matches('"');
    if trimmed.starts_with('/') {
        Some(PathBuf::from(trimmed))
    } else {
        None
    }
}

fn run_security(args: &[&str]) -> Result<std::process::Output, AgentCertError> {
    Command::new("/usr/bin/security")
        .args(args)
        .output()
        .map_err(|error| AgentCertError::Security(format!("无法启动 /usr/bin/security: {error}")))
}

fn security_failure(step: &str, output: &std::process::Output) -> AgentCertError {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    let stdout = String::from_utf8_lossy(&output.stdout);
    AgentCertError::Security(format!(
        "{step} 退出码 {}: {} {stderr}",
        output.status.code().unwrap_or(-1),
        stdout.trim()
    ))
}

/// 解析当前用户默认（login）keychain 路径。
///
/// # Errors
/// `security default-keychain` 失败或输出不可解析时返回 [`AgentCertError::Security`]。
pub fn login_keychain_path() -> Result<PathBuf, AgentCertError> {
    let output = run_security(&["default-keychain"])?;
    if !output.status.success() {
        return Err(security_failure("default-keychain", &output));
    }
    let text = String::from_utf8_lossy(&output.stdout).into_owned();
    parse_login_keychain_output(&text)
        .ok_or_else(|| AgentCertError::Security(format!("default-keychain 输出不可解析: {text}")))
}

/// 把根 CA 装入用户 login keychain 信任（用户级 trustRoot；系统可能弹确认，
/// 产品文案归 C3）。幂等性由调用方经 [`ca_installed_in_keychain`] 预判。
///
/// # Errors
/// `security add-trusted-cert` 启动失败或非零退出时返回 [`AgentCertError::Security`]。
pub fn install_trust_to_keychain(
    keychain: &Path,
    ca_pem_path: &Path,
) -> Result<(), AgentCertError> {
    let keychain_arg = keychain.display().to_string();
    let pem_arg = ca_pem_path.display().to_string();
    let output = run_security(&[
        "add-trusted-cert",
        "-r",
        "trustRoot",
        "-k",
        &keychain_arg,
        &pem_arg,
    ])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(security_failure("add-trusted-cert", &output))
    }
}

/// 列出钥匙串内 CN 匹配 [`CA_COMMON_NAME`] 的全部证书 DER（`security
/// find-certificate -a -c <CN> -p`）。无匹配返回空表。
///
/// # Errors
/// 进程启动失败或输出含不可解析 PEM 时返回 [`AgentCertError::Security`]。
pub fn keychain_ca_certificates() -> Result<Vec<CertificateDer<'static>>, AgentCertError> {
    let output = run_security(&["find-certificate", "-a", "-c", CA_COMMON_NAME, "-p"])?;
    if output.stdout.is_empty() {
        return Ok(Vec::new());
    }
    CertificateDer::pem_slice_iter(&output.stdout)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| {
            AgentCertError::Security(format!("find-certificate 输出不可解析: {error}"))
        })
}

/// 根 CA 是否已在钥匙串中（按 DER 逐字节等值比对，避免同名异钥误判）。
///
/// # Errors
/// [`keychain_ca_certificates`] 失败时透传。
pub fn ca_installed_in_keychain(material: &AgentCertMaterial) -> Result<bool, AgentCertError> {
    Ok(keychain_ca_certificates()?
        .iter()
        .any(|cert| cert.as_ref() == material.ca_der.as_slice()))
}

/// 删除单张受信证书（`security remove-trusted-cert <pem 文件>`）。
///
/// # Errors
/// 进程启动失败或非零退出时返回 [`AgentCertError::Security`]。
pub fn remove_trusted_cert_from_file(pem_path: &Path) -> Result<(), AgentCertError> {
    let pem_arg = pem_path.display().to_string();
    let output = run_security(&["remove-trusted-cert", &pem_arg])?;
    if output.status.success() {
        Ok(())
    } else {
        Err(security_failure("remove-trusted-cert", &output))
    }
}

/// 把 PEM 文本写到系统临时目录（0600 原子落盘），返回临时文件路径。
fn write_temp_certificate_pem(pem: &str, tag: &str) -> Result<PathBuf, AgentCertError> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        ".exv-agent-certs-{tag}-{}-{sequence}.pem",
        std::process::id()
    ));
    config_paths::atomic_write(&path, pem.as_bytes())
        .map_err(|error| AgentCertError::Io(format!("临时证书文件: {error}")))?;
    Ok(path)
}

/// 删除单张受信证书（PEM 文本输入；经临时文件转交 `security remove-trusted-cert`，
/// 结束后清理临时文件）。
///
/// # Errors
/// 临时文件写失败或 `remove-trusted-cert` 失败（含证书本就不在信任设置中）时返回
/// [`AgentCertError`]；是否把「不存在」视为已收敛由调用方决定。
pub fn remove_trusted_cert_pem(pem: &str) -> Result<(), AgentCertError> {
    let temp = write_temp_certificate_pem(pem, "remove")?;
    let result = remove_trusted_cert_from_file(&temp);
    let _ = std::fs::remove_file(&temp);
    result
}

/// 卸载清场：把钥匙串内 CN 匹配 [`CA_COMMON_NAME`] 的受信证书逐张导出并删除，
/// 再对持久化目录中的根 CA 做一次机会性补删（钥匙串导出与文件可能是同一张，按
/// DER 去重）。返回删除的张数。
///
/// 钥匙串导出候选的删除失败如实上报（它们确实在钥匙串里，删不掉是真实故障）；
/// 文件补删候选失败按「已收敛」跳过——该证书若不在信任设置中，本就没有可删对象。
/// 重复执行幂等收敛。
///
/// # Errors
/// [`keychain_ca_certificates`] 失败或钥匙串候选删除失败时返回 [`AgentCertError`]。
pub fn uninstall_agent_trust(cert_dir: Option<&Path>) -> Result<usize, AgentCertError> {
    let keychain_candidates = keychain_ca_certificates()?;
    let mut removed = 0;
    let mut seen_ders: Vec<Vec<u8>> = Vec::new();
    for candidate in &keychain_candidates {
        let der = candidate.as_ref();
        if seen_ders.iter().any(|seen| seen.as_slice() == der) {
            continue;
        }
        remove_trusted_cert_pem(&pem_from_der(der))?;
        seen_ders.push(der.to_vec());
        removed += 1;
    }
    if let Some(dir) = cert_dir
        && let Some(persisted) = load_material(dir)
        && !seen_ders
            .iter()
            .any(|seen| seen.as_slice() == persisted.ca_der)
    {
        let _ = remove_trusted_cert_pem(&pem_from_der(&persisted.ca_der));
    }
    Ok(removed)
}

/// DER → PEM（Base64 每行 64 字符，与 `security -p` 导出一致）。
fn pem_from_der(der: &[u8]) -> String {
    const LINE_WIDTH: usize = 64;
    use base64::Engine as _;
    let encoded = base64::engine::general_purpose::STANDARD.encode(der);
    let mut pem = String::with_capacity(encoded.len() + encoded.len() / LINE_WIDTH * 2 + 64);
    pem.push_str("-----BEGIN CERTIFICATE-----\n");
    for chunk in encoded.as_bytes().chunks(LINE_WIDTH) {
        pem.push_str(std::str::from_utf8(chunk).unwrap_or_default());
        pem.push('\n');
    }
    pem.push_str("-----END CERTIFICATE-----\n");
    pem
}

/// 钥匙串信任同步结果（`ensure_local_agent_tls` 的观测面）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTrustSync {
    /// 根 CA 已在钥匙串中（DER 等值），无需再装。
    AlreadyTrusted,
    /// 本次装入用户 login keychain 信任。
    Installed,
}

/// `ensure_local_agent_tls` 的交付物：服务端 TLS 配置 + 材料来源 + 信任同步结果。
pub struct LocalAgentTls {
    /// C1 `AgentServer` 接线用的 TLS 配置材料。
    pub server_config: AgentServerConfig,
    /// 落盘的证书材料。
    pub material: AgentCertMaterial,
    /// 证书来源（重生成时携带旧根 CA PEM）。
    pub cert_origin: AgentCertOrigin,
    /// 钥匙串信任同步结果。
    pub trust: AgentTrustSync,
}

/// 旧常驻入口的兼容接缝；已有用户根信任可直接准备，安装信任需本次确认。
/// 新请求编排调用 [`prepare_agent_certificate`] 传递本次确认结果。
///
/// # Errors
/// 需要确认或证书/钥匙串失败时返回真实准备错误。
pub fn ensure_local_agent_tls(dir: &Path) -> Result<LocalAgentTls, AgentCertError> {
    prepare_agent_certificate(dir, false).map(|prepared| prepared.tls)
}

/// 本次系统浏览器证书检查结果；检查阶段只读。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertificatePreparation {
    Ready,
    ApprovalRequired { reason: String },
}

/// 当前用户钥匙串信任端口，测试只注入隔离实现。
pub trait CertificateTrustStore {
    fn is_trusted(&self, ca_der: &[u8]) -> Result<bool, AgentCertError>;
    fn install(&self, ca_pem: &Path) -> Result<(), AgentCertError>;
}

struct UserKeychainTrustStore<'a> {
    dir: &'a Path,
}
impl CertificateTrustStore for UserKeychainTrustStore<'_> {
    fn is_trusted(&self, ca_der: &[u8]) -> Result<bool, AgentCertError> {
        if !keychain_ca_certificates()?
            .iter()
            .any(|cert| cert.as_ref() == ca_der)
        {
            return Ok(false);
        }
        // 证书存在与用户实际信任分开检查；verify-cert 使用系统信任评估。
        let path = root_ca_pem_path(self.dir).display().to_string();
        Ok(run_security(&["verify-cert", "-c", &path])?
            .status
            .success())
    }
    fn install(&self, ca_pem: &Path) -> Result<(), AgentCertError> {
        install_trust_to_keychain(&login_keychain_path()?, ca_pem)
    }
}

/// 已准备好的 TLS 及沿用原目录的文件路径。
pub struct PreparedAgentTls {
    pub cert_path: PathBuf,
    pub key_path: PathBuf,
    pub tls: LocalAgentTls,
}

fn existing_root(dir: &Path) -> Option<(String, Vec<u8>)> {
    let pem = persisted_root_ca_pem(dir)?;
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

/// 只读检查当前目录的材料和用户信任，不生成、写入或删除证书。
pub fn inspect_agent_certificate_with_store(
    dir: &Path,
    store: &dyn CertificateTrustStore,
) -> Result<CertificatePreparation, AgentCertError> {
    if let Some((_, root)) = existing_root(dir) {
        let usable = load_material(dir)
            .is_some_and(|material| material_usable_at(&material, UnixTime::now()));
        if store.is_trusted(&root)? && (usable || root_signing_key(dir, &root).is_some()) {
            return Ok(CertificatePreparation::Ready);
        }
    }
    Ok(CertificatePreparation::ApprovalRequired {
        reason: "系统浏览器认证需要安装 EXV 本地登录证书。继续后，系统将请求把证书加入当前用户的信任存储。".into(),
    })
}

/// 使用真实用户钥匙串，只读检查证书。
pub fn inspect_agent_certificate(dir: &Path) -> Result<CertificatePreparation, AgentCertError> {
    inspect_agent_certificate_with_store(dir, &UserKeychainTrustStore { dir })
}

/// 在本次用户确认后准备 TLS；已有根信任只更新叶证书。
pub fn prepare_agent_certificate(
    dir: &Path,
    approved: bool,
) -> Result<PreparedAgentTls, AgentCertError> {
    prepare_agent_certificate_with_store(dir, &UserKeychainTrustStore { dir }, approved)
}

pub fn prepare_agent_certificate_with_store(
    dir: &Path,
    store: &dyn CertificateTrustStore,
    approved: bool,
) -> Result<PreparedAgentTls, AgentCertError> {
    if let CertificatePreparation::ApprovalRequired { reason } =
        inspect_agent_certificate_with_store(dir, store)?
    {
        if !approved {
            return Err(AgentCertError::ApprovalRequired { reason });
        }
    }
    let (material, cert_origin, needs_persist) =
        match load_material(dir).filter(|material| material_usable_at(material, UnixTime::now())) {
            Some(material) => (material, AgentCertOrigin::Reused, false),
            None => {
                let root = existing_root(dir).and_then(|(pem, der)| {
                    let key = root_signing_key(dir, &der)?;
                    Some((pem, der, key))
                });
                if let Some((pem, der, key)) = root {
                    let mut material =
                        generate_material_with_ca_key_at(&key, time::OffsetDateTime::now_utc())?;
                    material.ca_pem = pem;
                    material.ca_der = der;
                    if !material_usable_at(&material, UnixTime::now()) {
                        return Err(AgentCertError::Corrupt("原根证书验签失败".into()));
                    }
                    (material, AgentCertOrigin::Reused, true)
                } else {
                    let previous = persisted_root_ca_pem(dir);
                    let material = generate_material()?;
                    (
                        material,
                        AgentCertOrigin::Generated {
                            replaced_ca_pem: previous,
                        },
                        true,
                    )
                }
            }
        };
    let trust = if store.is_trusted(&material.ca_der)? {
        AgentTrustSync::AlreadyTrusted
    } else {
        if !approved {
            return Err(AgentCertError::ApprovalRequired {
                reason: "系统浏览器认证需要安装 EXV 本地登录证书。".into(),
            });
        }
        if needs_persist {
            install_pending_root(dir, &material, store)?;
        } else {
            store.install(&root_ca_pem_path(dir))?;
        }
        AgentTrustSync::Installed
    };
    if needs_persist {
        persist_material(dir, &material)?;
    }
    Ok(PreparedAgentTls {
        cert_path: leaf_cert_pem_path(dir),
        key_path: leaf_key_pem_path(dir),
        tls: LocalAgentTls {
            server_config: AgentServerConfig::from_material(&material),
            material,
            cert_origin,
            trust,
        },
    })
}

// 安装仅消费公开根 PEM；新叶和私钥留在内存，成功后再发布到固定路径。
fn install_pending_root(
    dir: &Path,
    material: &AgentCertMaterial,
    store: &dyn CertificateTrustStore,
) -> Result<(), AgentCertError> {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!(".pending-ca-{}-{sequence}.pem", std::process::id()));
    config_paths::atomic_write(&path, material.ca_pem.as_bytes())
        .map_err(|error| AgentCertError::Io(format!("临时根证书: {error}")))?;
    let result = store.install(&path);
    let _ = std::fs::remove_file(&path);
    result
}

