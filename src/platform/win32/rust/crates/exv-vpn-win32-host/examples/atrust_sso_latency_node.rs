//! 使用正式 SSO CAS 兑换，为无网卡节点实验准备会话；不写正式档案或配置。
//! 参数：--gateway HTTPS_ORIGIN --profile DIR --physical-ifindex N -- [节点实验键值参数]
use exv_core::{
    atrust_cas_login::{CasLoginClient, CasLoginOptions, CasOutcome},
    sso_session_jar,
};
use exv_engine::atrust_underlay::PhysicalUnderlay;
use exv_vpn_atrust::auth::{
    browser_session::{BrowserSession, session_from_cookie_pairs},
    origin::GatewayOrigin,
};
use std::{
    future::Future,
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::AsyncWriteExt,
    process::{Child, Command},
    time::Instant,
};
use zeroize::Zeroizing;

#[allow(dead_code)]
#[path = "../../../../../../common/rust/crates/exv-vpn-atrust/examples/support/latency_lab/config.rs"]
mod lab_config;

const CHILD_BUDGET: Duration = Duration::from_secs(125);

struct Options {
    gateway: String,
    profile: PathBuf,
    physical_ifindex: u32,
    node_args: Vec<String>,
}

impl Options {
    fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, &'static str> {
        let mut args = args.into_iter();
        let (mut gateway, mut profile, mut physical_ifindex) = (None, None, None);
        let mut node_args = Vec::new();
        while let Some(key) = args.next() {
            if key == "--" {
                node_args.extend(args);
                break;
            }
            let value = args.next().ok_or("argument_value_missing")?;
            match key.as_str() {
                "--gateway" if gateway.is_none() => {
                    GatewayOrigin::parse(&value).map_err(|_| "invalid_gateway")?;
                    gateway = Some(value);
                }
                "--profile" if profile.is_none() && !value.is_empty() => {
                    profile = Some(PathBuf::from(value))
                }
                "--physical-ifindex" if physical_ifindex.is_none() => {
                    physical_ifindex = Some(
                        value
                            .parse::<u32>()
                            .ok()
                            .filter(|n| *n > 0)
                            .ok_or("invalid_interface")?,
                    );
                }
                _ => return Err("unknown_or_duplicate_owner_argument"),
            }
        }
        // 模式、网关和物理出口只有本入口一个权威，节点实验参数沿用原解析器。
        if node_args.iter().any(|arg| {
            matches!(
                arg.as_str(),
                "--mode" | "--gateway" | "--profile" | "--physical-ifindex"
            )
        }) {
            return Err("node_argument_overrides_owner");
        }
        let mut validated = vec!["--mode".into(), "node".into()];
        validated.extend(node_args.clone());
        let config = lab_config::Config::parse(validated).map_err(|_| "invalid_node_arguments")?;
        if config.protocol == "icmp" && (config.flows > 1 || config.scenario == "new-flow") {
            return Err("unsupported_icmp_flows");
        }
        Ok(Self {
            gateway: gateway.ok_or("gateway_missing")?,
            profile: profile.ok_or("profile_missing")?,
            physical_ifindex: physical_ifindex.ok_or("interface_missing")?,
            node_args,
        })
    }

    fn child_args(&self) -> Vec<String> {
        let mut args = vec![
            "--mode".into(),
            "node".into(),
            "--gateway".into(),
            self.gateway.clone(),
            "--physical-ifindex".into(),
            self.physical_ifindex.to_string(),
        ];
        args.extend(self.node_args.clone());
        args
    }
}

fn node_executable(current_exe: &Path) -> Result<PathBuf, &'static str> {
    Ok(current_exe
        .parent()
        .ok_or("executable_directory_missing")?
        .join("atrust_latency_node.exe"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PreparationError {
    result: &'static str,
    cas: Option<exv_core::atrust_cas_login::CasNetError>,
}

impl From<&'static str> for PreparationError {
    fn from(result: &'static str) -> Self {
        Self { result, cas: None }
    }
}

fn session_from_outcome(
    gateway: &str,
    user_agent: &str,
    outcome: CasOutcome,
) -> Result<BrowserSession, PreparationError> {
    match outcome {
        CasOutcome::Ticket {
            sid_via_auth_check: Some(exchange),
            ..
        } if !exchange.is_empty() => {
            // 与正式 HttpProbeAdapter 的会话组装同源；每枚字段仍须经过既有验证。
            let header = exchange.cookie_header();
            let pairs = header
                .split(';')
                .filter_map(|part| part.trim().split_once('='))
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect();
            session_from_cookie_pairs(gateway, user_agent, pairs)
                .map_err(|_| "cas_session_invalid".into())
        }
        CasOutcome::Ticket { .. } => Err("cas_session_missing".into()),
        CasOutcome::FormRequired => Err("cas_interaction_required".into()),
        CasOutcome::NetError(cas) => Err(PreparationError {
            result: "cas_network_or_protocol_error",
            cas: Some(cas),
        }),
    }
}

async fn prepare(options: &Options) -> Result<Zeroizing<Vec<u8>>, PreparationError> {
    let mut jar = sso_session_jar::load(&options.profile)
        .map_err(|_| "sso_storage_error")?
        .ok_or("sso_missing")?;
    if !jar.has_cookie("SESSION") || !jar.has_cookie("SOURCEID_TGC") {
        return Err("sso_incomplete".into());
    }
    let underlay = Arc::new(
        PhysicalUnderlay::new(options.physical_ifindex).map_err(|_| "physical_underlay_error")?,
    );
    let config = CasLoginOptions::default();
    let user_agent = config.user_agent.clone();
    let mut client = CasLoginClient::new(&options.gateway, underlay, config)
        .map_err(|_| "cas_configuration_error")?;
    let outcome = tokio::time::timeout(Duration::from_secs(10), client.login(&mut jar))
        .await
        .map_err(|_| "cas_timeout")?;
    let session = session_from_outcome(&options.gateway, &user_agent, outcome)?;
    // 本轮 SSO cookie 轮换只留在内存；不调用任何正式缓存/配置的 save。
    session
        .encode()
        .map_err(|_| "cas_session_encode_error".into())
}

async fn supervise_child<F>(
    child: &mut Child,
    secret: Zeroizing<Vec<u8>>,
    deadline: Instant,
    cancel: &mut F,
) -> Result<(), &'static str>
where
    F: Future<Output = std::io::Result<()>> + Unpin,
{
    let operation = async {
        let mut stdin = child.stdin.take().ok_or("node_stdin_missing")?;
        stdin
            .write_all(&secret)
            .await
            .map_err(|_| "node_stdin_write_error")?;
        stdin
            .shutdown()
            .await
            .map_err(|_| "node_stdin_close_error")?;
        drop(stdin);
        drop(secret);
        let status = child.wait().await.map_err(|_| "node_wait_error")?;
        if status.success() {
            Ok(())
        } else {
            Err("node_failed")
        }
    };
    let result = tokio::select! {
        result = tokio::time::timeout_at(deadline, operation) => result.unwrap_or(Err("node_timeout")),
        _ = cancel => Err("cancelled"),
    };
    // 所有错误/超时/取消路径明确结束并收割；kill_on_drop 仅用于异常展开兜底。
    if result.is_err() {
        let killed = child.kill().await;
        let waited = child.wait().await;
        if waited.is_err() || (killed.is_err() && child.id().is_some()) {
            return Err("node_cleanup_failed");
        }
    }
    result
}

fn prep_record(result: Result<(), PreparationError>, elapsed: Duration) -> String {
    let error = result.err();
    let mut record = serde_json::json!({
        "stage":"sso-preparation",
        "result": error.map_or("ready", |error| error.result),
        "elapsed_ms":elapsed.as_millis()
    });
    if let Some(cas) = error.and_then(|error| error.cas) {
        // 正式 CAS 类型只携带稳定分类与 HTTP 状态，不包含服务器正文或秘密。
        record["cas_code"] = serde_json::json!(cas.code);
        if let Some(status) = cas.http_status {
            record["http_status"] = serde_json::json!(status);
        }
    }
    record.to_string()
}

#[tokio::main]
async fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let result = run().await;
    if let Err(code) = result {
        eprintln!(
            "{}",
            serde_json::json!({"stage":"sso-node-adapter","result":code})
        );
        std::process::exit(1);
    }
}

async fn run() -> Result<(), &'static str> {
    let options = Options::parse(std::env::args().skip(1))?;
    let executable =
        node_executable(&std::env::current_exe().map_err(|_| "current_executable_error")?)?;
    if !executable.is_file() {
        return Err("node_executable_missing");
    }
    let mut cancel = Box::pin(tokio::signal::ctrl_c());
    let started = Instant::now();
    let session = tokio::select! {
        result = prepare(&options) => result,
        _ = &mut cancel => Err("cancelled".into()),
    };
    eprintln!(
        "{}",
        prep_record(
            session.as_ref().map(|_| ()).map_err(|code| *code),
            started.elapsed()
        )
    );
    let session = session.map_err(|error| error.result)?;
    let deadline = Instant::now() + CHILD_BUDGET;
    // stdout 只继承固定节点程序的 JSONL；秘密只经匿名 stdin，不经参数或磁盘。
    let mut child = Command::new(executable)
        .args(options.child_args())
        .stdin(Stdio::piped())
        .stdout(Stdio::inherit())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| "node_spawn_error")?;
    supervise_child(&mut child, session, deadline, &mut cancel).await
}
