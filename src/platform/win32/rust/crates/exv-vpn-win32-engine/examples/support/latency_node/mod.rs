use crate::latency_lab::{
    config::Config,
    records::Ledger,
    runtime::{ExchangeResult, exchange, final_record},
};
use exv_engine::{
    atrust_auth::authenticate_browser_session, atrust_node_tls::connect_node,
    atrust_tunnel::NodeIdentity, atrust_underlay::PhysicalUnderlay,
};
use exv_vpn_atrust::{
    ConnectPurpose, OutboundTarget, Secret,
    auth::origin::GatewayOrigin,
    resource::{Dispatch, NodeClass, ResourceCatalog, Target},
    session::{Session, SessionFactory},
    transport::l3::{
        connection::{ConnectionOptions, HeartbeatPolicy, L3Connection},
        packet_engine::{FlowIdentity, FlowLimits, PacketEngine},
    },
};
use exv_vpn_win32_ipc::browser_session::BrowserSession;
use serde_json::{Value, json};
use std::{
    io::Read,
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::time::Instant as Deadline;
use zeroize::Zeroizing;

pub struct Options {
    pub config: Config,
    pub gateway: String,
    pub physical_ifindex: u32,
}
impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut args = args.into_iter();
        let mut remaining = Vec::new();
        let mut gateway = None;
        let mut physical_ifindex = None;
        while let Some(key) = args.next() {
            let value = args.next().ok_or("节点参数缺少取值")?;
            match key.as_str() {
                "--gateway" if gateway.is_none() => {
                    GatewayOrigin::parse(&value).map_err(|_| "节点网关必须是 HTTPS origin")?;
                    gateway = Some(value);
                }
                "--physical-ifindex" if physical_ifindex.is_none() => {
                    physical_ifindex = Some(
                        value
                            .parse::<u32>()
                            .ok()
                            .filter(|n| *n > 0)
                            .ok_or("物理接口索引必须为正整数")?,
                    );
                }
                "--gateway" | "--physical-ifindex" => return Err("节点参数重复".into()),
                _ => {
                    remaining.push(key);
                    remaining.push(value);
                }
            }
        }
        let config = Config::parse(remaining)?;
        if config.mode != "node" {
            return Err("真实节点入口要求显式 --mode node".into());
        }
        if config.protocol == "icmp" && (config.flows > 1 || config.scenario == "new-flow") {
            return Err("ICMP 多授权流实验不受支持".into());
        }
        Ok(Self {
            config,
            gateway: gateway.ok_or("节点网关缺失")?,
            physical_ifindex: physical_ifindex.ok_or("物理接口索引缺失")?,
        })
    }
}

struct Selection {
    node: OutboundTarget,
    group: String,
    bundled_trust: bool,
}
fn select(catalog: &ResourceCatalog, options: &Options) -> Result<Selection, &'static str> {
    let target = if options.config.protocol == "dns" {
        Target::udp(options.config.target, 53)
    } else {
        Target::icmp(options.config.target)
    };
    let Dispatch::L3(resource) = catalog.dispatch(target) else {
        return Err("target_not_covered_by_l3_catalog");
    };
    let group = resource
        .node_group_id
        .ok_or("target_resource_node_group_missing")?;
    let addresses = catalog
        .node_group_addresses(&group)
        .ok_or("target_node_group_missing")?;
    let origin = GatewayOrigin::parse(&options.gateway).map_err(|_| "invalid_gateway")?;
    let bundled_trust = origin.host() == "vpn.ecnu.edu.cn" && origin.port() == 443;
    let node = if bundled_trust {
        addresses.iter().find(|node| {
            node.class == NodeClass::Wan && node.host == "222.66.117.110" && node.port == 441
        })
    } else {
        addresses
            .iter()
            .find(|node| node.class == NodeClass::Wan)
            .or_else(|| addresses.first())
    }
    .ok_or("authorized_node_with_production_trust_missing")?;
    Ok(Selection {
        node: OutboundTarget::new(node.host.clone(), node.port, ConnectPurpose::DataNode),
        group,
        bundled_trust,
    })
}

fn read_browser(reader: impl Read) -> Result<BrowserSession, &'static str> {
    let mut bytes = Zeroizing::new(Vec::new());
    reader
        .take(131_073)
        .read_to_end(&mut bytes)
        .map_err(|_| "browser_stdin_read")?;
    if bytes.len() > 131_072 {
        return Err("browser_stdin_too_large");
    }
    BrowserSession::decode(&bytes).map_err(|_| "browser_stdin_invalid")
}

pub async fn run(options: Options, cancel: Session, emit: &mut dyn FnMut(Value)) -> bool {
    let started = Instant::now();
    let deadline = Deadline::now() + Duration::from_millis(options.config.timeout_ms);
    let run_id = uuid::Uuid::new_v4().to_string();
    let mut output = |mut value: Value| {
        value["schema_version"] = json!(1);
        value["run_id"] = json!(run_id);
        emit(value);
    };
    let mut ledger = Ledger::default();
    let mut parameters = serde_json::to_value(&options.config).expect("实验参数序列化");
    parameters["actual_nodelay"] = Value::Null;
    parameters["peer_actual_nodelay"] = Value::Null;
    parameters["heartbeat_effective"] = Value::Null;
    parameters["physical_ifindex"] = json!(options.physical_ifindex);
    parameters["effective_window"] = json!(if matches!(
        options.config.scenario.as_str(),
        "echo" | "new-flow"
    ) {
        1
    } else {
        options.config.window
    });
    let source_revision = crate::latency_lab::runtime::source_revision();
    let lifecycle = SessionFactory::new().open();
    let setup_started = Instant::now();
    let mut failure_detail = Value::Null;
    let mut tcp_observation = crate::node_tcp_observation::NodeTcpObservation::default();
    let setup = async {
        // 启动器关闭写端即结束输入；直接调用时仍受本轮 deadline 和取消约束。
        let mut bytes = Zeroizing::new(Vec::new());
        let mut input = tokio::io::AsyncReadExt::take(tokio::io::stdin(), 131_073);
        tokio::io::AsyncReadExt::read_to_end(&mut input, &mut bytes)
            .await
            .map_err(|_| "browser_stdin_read")?;
        let browser = read_browser(bytes.as_slice())?;
        drop(bytes);
        let underlay = Arc::new(PhysicalUnderlay::new_with_node_observer(
            options.physical_ifindex,
            options.config.nodelay,
            tcp_observation.observer(),
        )?);
        let authenticated =
            authenticate_browser_session(browser, &options.gateway, underlay.clone())
                .await
                .map_err(|error| {
                    failure_detail = json!(error.to_string());
                    "browser_authentication_failed"
                })?;
        let selected = select(authenticated.catalog(), &options)?;
        let observed = selected.bundled_trust.then_some(
            include_bytes!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/resources/ecnu-atrust-node.der"
            ))
            .as_slice(),
        );
        let tls = connect_node(underlay.as_ref(), &selected.node, "sdp.ssl.com", observed)
            .await
            .map_err(|error| {
                failure_detail = json!(error);
                "node_tls_failed"
            })?;
        let actual_nodelay = underlay
            .actual_node_nodelay()
            .ok_or("node_nodelay_readback_missing")?;
        let identity = NodeIdentity::for_current_process().map_err(|_| "node_identity_failed")?;
        let sid = Secret::new(
            authenticated
                .sid()
                .ok_or("authenticated_sid_missing")?
                .expose(),
        );
        let policy = if options.config.heartbeat == "idle" {
            HeartbeatPolicy::IdleOnly
        } else {
            HeartbeatPolicy::Periodic
        };
        let handshake_started = Instant::now();
        let connection = L3Connection::open_with_heartbeat_policy(
            tls,
            &sid,
            lifecycle.clone(),
            ConnectionOptions::default(),
            policy,
        )
        .await
        .map_err(|error| {
            failure_detail = json!(error.to_string());
            "node_l3_handshake_failed"
        })?;
        let handshake_us = handshake_started.elapsed().as_micros() as u64;
        let engine = PacketEngine::new(
            connection.vip(),
            selected.group.clone(),
            authenticated.catalog().clone(),
            FlowIdentity {
                sid,
                sign_key: identity.sign_key,
                device_id: identity.device_id,
                connection_id: identity.connection_id,
                process_hash: identity.process_hash,
                environment: identity.environment,
            },
            FlowLimits::default(),
        );
        Ok::<_, &'static str>((connection, engine, selected, actual_nodelay, handshake_us))
    };
    let opening = tokio::select! { biased; ()=cancel.cancelled()=>Err("cancelled"), result=tokio::time::timeout_at(deadline,setup)=>match result {Ok(v)=>v,Err(_)=>Err("setup_timeout")} };
    let setup_us = setup_started.elapsed().as_micros() as u64;
    let start = |parameters: Value| json!({"record_type":"start","mode":"node","host":"windows","scenario":options.config.scenario,"candidate":options.config.candidate(),"source_revision":source_revision,"parameters":parameters,"evidence_layer":"real_node_raw_ipv4"});
    let mut handshake_us = Value::Null;
    let result = match opening {
        Ok((connection, engine, selected, actual, handshake)) => {
            parameters["actual_nodelay"] = json!(actual);
            parameters["heartbeat_effective"] = json!(options.config.heartbeat);
            parameters["selected_node"] = json!({"host":selected.node.host,"port":selected.node.port,"group":selected.group,"tls_server_name":"sdp.ssl.com","bundled_trust":selected.bundled_trust});
            handshake_us = json!(handshake);
            output(start(parameters));
            exchange(
                connection,
                engine,
                &options.config,
                cancel,
                &mut output,
                &mut ledger,
                deadline,
            )
            .await
        }
        Err(reason) => {
            output(start(parameters));
            ExchangeResult {
                outcome: if reason == "cancelled" {
                    "cancelled"
                } else {
                    "failed"
                },
                cleanup: true,
                close_us: 0,
                reason: Some(reason),
                cleanup_tasks: json!({"forwarding":null,"consumer":null}),
            }
        }
    };
    let _ = lifecycle.close();
    // 真实DataNode的duplicate也须stop/join/drop后才签本轮cleanup。
    let (outer_tcp, observer_cleanup) = tcp_observation.finish();
    let cleanup = result.cleanup && observer_cleanup;
    let outcome = if cleanup { result.outcome } else { "failed" };
    output(final_record(
        &ledger,
        outcome,
        cleanup,
        started.elapsed(),
        json!({"setup_us":setup_us,"handshake_us":handshake_us,"authorization_us":null,"queue_residence_us":null,"write_wait_us":null,"allocation_count":null,"copy_bytes":null,"close_us":result.close_us,"reason":result.reason,"cleanup_tasks":result.cleanup_tasks,"failure_detail":failure_detail,"outbound_ipv4_bytes":ledger.entries.iter().map(|entry|entry.packet.len()).sum::<usize>(),"unmatched":ledger.unmatched,"tcp_io":null,"outer_tcp":outer_tcp}),
    ));
    outcome == "passed" && cleanup
}
