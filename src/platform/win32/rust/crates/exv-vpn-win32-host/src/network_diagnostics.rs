//! Windows 连接边界的只读网络诊断；所有原生查询都在有界 blocking 任务中执行。

use std::collections::BTreeMap;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use exv_vpn_win32_ipc::peer_auth::current_user_sid;
use exv_vpn_win32_resource::{proxy_tun, routes, system_proxy, vgdc_dns};
use serde_json::{Value, json};
use tokio::sync::Semaphore;
use uuid::Uuid;
use windows::Win32::NetworkManagement::IpHelper::{
    FreeMibTable, GAA_FLAG_INCLUDE_GATEWAYS, GetAdaptersAddresses, GetIpForwardTable2,
    IP_ADAPTER_ADDRESSES_LH, MIB_IPFORWARD_TABLE2,
};
use windows::Win32::Networking::WinSock::{
    AF_INET, AF_INET6, AF_UNSPEC, SOCKADDR_IN, SOCKADDR_IN6, SOCKET_ADDRESS,
};

use crate::log_aggregator::LogAggregator;
use crate::process_lifecycle::ENGINE_ADAPTER_NAME;

const CAPTURE_DEADLINE: Duration = Duration::from_secs(5);
const MAX_INTERFACES: usize = 32;
const MAX_ADDRESSES: usize = 8;
const MAX_ROUTES: usize = 128;
const MAX_TEXT_BYTES: usize = 192;
const MAX_ADAPTER_BUFFER_BYTES: u32 = 4 * 1024 * 1024;
static CAPTURE_GATE: OnceLock<Arc<Semaphore>> = OnceLock::new();
const MONITOR_PERIOD: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, PartialEq, Eq)]
struct MonitorKey {
    operation_id: Uuid,
    engine_generation: u64,
}

#[derive(Clone)]
struct CachedNetwork {
    facts: Value,
    finished: Instant,
    started_ms: u128,
    finished_ms: u128,
    duration_ms: u128,
}

#[derive(Default)]
struct MonitorState {
    revision: u64,
    key: Option<MonitorKey>,
    cached: Option<CachedNetwork>,
}

/// 只由状态转发器持有；销毁或取消即 abort timer，原生在途工作仍受全局单槽约束。
#[derive(Default)]
pub(crate) struct NetworkMonitor {
    state: Arc<Mutex<MonitorState>>,
    task: Option<tokio::task::AbortHandle>,
}

impl Drop for NetworkMonitor {
    fn drop(&mut self) {
        self.cancel();
    }
}

impl NetworkMonitor {
    pub(crate) fn cancel(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.revision = state.revision.wrapping_add(1);
        state.key = None;
        state.cached = None;
    }

    pub(crate) fn invalidate_if_different(&mut self, operation_id: Option<Uuid>, generation: u64) {
        let different = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .key
            .is_some_and(|key| {
                Some(key.operation_id) != operation_id || key.engine_generation != generation
            });
        if different {
            self.cancel();
        }
    }

    pub(crate) fn start(
        &mut self,
        logs: Arc<LogAggregator>,
        context: SnapshotContext,
        operation_id: Uuid,
        generation: u64,
    ) {
        let gate = Arc::clone(CAPTURE_GATE.get_or_init(|| Arc::new(Semaphore::new(1))));
        self.start_sampling(
            logs,
            context,
            MonitorKey {
                operation_id,
                engine_generation: generation,
            },
            gate,
            MONITOR_PERIOD,
            Arc::new(collect_light_network),
        );
    }

    fn start_sampling(
        &mut self,
        logs: Arc<LogAggregator>,
        context: SnapshotContext,
        key: MonitorKey,
        gate: Arc<Semaphore>,
        period: Duration,
        probe: Arc<dyn Fn(Instant) -> Value + Send + Sync>,
    ) {
        if self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .key
            == Some(key)
        {
            return;
        }
        self.cancel();
        let revision = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.key = Some(key);
            state.revision
        };
        let state = Arc::downgrade(&self.state);
        let task = tokio::spawn(async move {
            let mut previous_outcome = None;
            loop {
                if state.strong_count() == 0 {
                    return;
                }
                // 满载不排队；连续相同跳过原因只记录首次，下个周期再试。
                if let Ok(permit) = Arc::clone(&gate).try_acquire_owned() {
                    let probe = Arc::clone(&probe);
                    let started = Instant::now();
                    let deadline = started + CAPTURE_DEADLINE;
                    let worker = tokio::task::spawn_blocking(move || {
                        let _permit = permit;
                        let started_ms = wall_ms();
                        let facts = probe(deadline);
                        CachedNetwork {
                            facts,
                            finished: Instant::now(),
                            started_ms,
                            finished_ms: wall_ms(),
                            duration_ms: started.elapsed().as_millis(),
                        }
                    });
                    match tokio::time::timeout(CAPTURE_DEADLINE, worker).await {
                        Ok(Ok(sample)) => {
                            previous_outcome = Some("completed");
                            if let Some((code, fields)) =
                                commit_monitor_sample(&state, key, revision, sample, &context)
                            {
                                let _ = logs.append_core(
                                    "debug",
                                    "network",
                                    code,
                                    "连接期间的网络快照",
                                    &fields,
                                );
                            }
                        }
                        Ok(Err(_)) => emit_monitor_skip(
                            &logs,
                            &context,
                            "worker_error",
                            &mut previous_outcome,
                        ),
                        Err(_) => {
                            emit_monitor_skip(&logs, &context, "timeout", &mut previous_outcome)
                        }
                    }
                } else {
                    emit_monitor_skip(&logs, &context, "busy", &mut previous_outcome);
                }
                // 完成后再等 period，慢探测不会产生追赶补采或密集日志。
                tokio::time::sleep(period).await;
            }
        });
        self.task = Some(task.abort_handle());
    }

    /// 先冻结内存中的此前观测，再取消；不等待正在运行的原生查询。
    pub(crate) fn record_disconnect(&mut self, logs: &LogAggregator, context: &SnapshotContext) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let cached = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            state.revision = state.revision.wrapping_add(1);
            state.key = None;
            state.cached.take()
        };
        let mut fields = context.fields.clone();
        fields.insert(
            "observation_relation".into(),
            "previous_observation_not_failure_instant".into(),
        );
        if let Some(cached) = cached {
            fields.insert("cache_status".into(), "available".into());
            fields.insert(
                "cache_age_ms".into(),
                cached.finished.elapsed().as_millis().to_string(),
            );
            append_sample_fields(&mut fields, &cached);
        } else {
            fields.insert("cache_status".into(), "unknown".into());
            fields.insert(
                "cache_reason".into(),
                "no_completed_sample_for_current_connection".into(),
            );
        }
        let _ = logs.append_core(
            "debug",
            "network",
            "kernel.network.disconnect_cached",
            "断开时附带最近一次网络快照",
            &fields,
        );
    }

    pub(crate) fn record_stream_disconnect(&mut self, logs: &LogAggregator, generation: u64) {
        let key = self
            .state
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .key;
        if let Some(key) = key.filter(|key| key.engine_generation == generation) {
            let context = SnapshotContext::new(
                "status_stream_eof",
                Some(key.operation_id),
                None,
                0,
                generation,
            );
            self.record_disconnect(logs, &context);
        } else {
            self.cancel();
        }
    }
}

fn emit_monitor_skip(
    logs: &LogAggregator,
    context: &SnapshotContext,
    outcome: &'static str,
    previous: &mut Option<&'static str>,
) {
    if *previous == Some(outcome) {
        return;
    }
    *previous = Some(outcome);
    let mut fields = context.fields.clone();
    fields.insert("outcome".into(), outcome.into());
    fields.insert("observed_ms".into(), wall_ms().to_string());
    fields.insert("cache_updated".into(), "false".into());
    let _ = logs.append_core(
        "debug",
        "network",
        "kernel.network.monitor_sample_skipped",
        "本轮网络采样未完成，沿用上次结果",
        &fields,
    );
}

fn append_sample_fields(fields: &mut BTreeMap<String, String>, sample: &CachedNetwork) {
    fields.insert("sample_started_ms".into(), sample.started_ms.to_string());
    fields.insert("sample_finished_ms".into(), sample.finished_ms.to_string());
    fields.insert("sample_duration_ms".into(), sample.duration_ms.to_string());
    fields.insert("snapshot".into(), sample.facts.to_string());
}

fn commit_monitor_sample(
    state: &Weak<Mutex<MonitorState>>,
    key: MonitorKey,
    revision: u64,
    sample: CachedNetwork,
    context: &SnapshotContext,
) -> Option<(&'static str, BTreeMap<String, String>)> {
    let state = state.upgrade()?;
    let mut state = state.lock().unwrap_or_else(|error| error.into_inner());
    if state.key != Some(key) || state.revision != revision {
        return None;
    }
    let previous = state.cached.replace(sample.clone());
    drop(state);
    let mut fields = context.fields.clone();
    append_sample_fields(&mut fields, &sample);
    fields.insert(
        "observation_relation".into(),
        "connected_periodic_observation".into(),
    );
    match previous {
        None => Some(("kernel.network.connected_baseline", fields)),
        Some(previous) if previous.facts != sample.facts => {
            let changed: Vec<_> = ["adapters", "key_routes", "system_proxy"]
                .into_iter()
                .filter(|key| previous.facts[*key] != sample.facts[*key])
                .collect();
            fields.insert("changed_sections".into(), json!(changed).to_string());
            fields.insert(
                "previous_sample_finished_ms".into(),
                previous.finished_ms.to_string(),
            );
            // 前后均为三类有界事实；没有变化时只刷新缓存时间，不重复日志。
            fields.insert("previous_snapshot".into(), previous.facts.to_string());
            Some(("kernel.network.environment_changed", fields))
        }
        Some(_) => None,
    }
}

/// 只含操作身份和时间，不接受连接配置或凭据。
#[derive(Clone)]
pub(crate) struct SnapshotContext {
    fields: BTreeMap<String, String>,
    requested: Instant,
}

impl SnapshotContext {
    pub(crate) fn new(
        trigger: &'static str,
        operation_id: Option<Uuid>,
        attempt_id: Option<Uuid>,
        reconnect_attempt: u32,
        engine_generation: u64,
    ) -> Self {
        let mut context = Self {
            fields: BTreeMap::from([
                ("snapshot_id".into(), Uuid::new_v4().to_string()),
                ("trigger".into(), trigger.into()),
                (
                    "operation_id".into(),
                    operation_id.map_or_else(|| "unknown".into(), |id| id.to_string()),
                ),
                (
                    "attempt_id".into(),
                    attempt_id.map_or_else(|| "unknown".into(), |id| id.to_string()),
                ),
                ("reconnect_attempt".into(), reconnect_attempt.to_string()),
                ("engine_generation".into(), engine_generation.to_string()),
                ("core_pid".into(), std::process::id().to_string()),
                (
                    "core_package_version".into(),
                    env!("CARGO_PKG_VERSION").into(),
                ),
                ("requested_ms".into(), wall_ms().to_string()),
            ]),
            requested: Instant::now(),
        };
        context.fields.extend(crate::build_identity::fields());
        context
    }
}

/// 调用方不等待探测；满载时记录 busy，绝不排队积累原生查询。
pub(crate) fn request_snapshot(logs: Arc<LogAggregator>, context: SnapshotContext) {
    let gate = Arc::clone(CAPTURE_GATE.get_or_init(|| Arc::new(Semaphore::new(1))));
    drop(spawn_capture(
        logs,
        context,
        gate,
        CAPTURE_DEADLINE,
        collect_network,
    ));
}

fn spawn_capture(
    logs: Arc<LogAggregator>,
    context: SnapshotContext,
    gate: Arc<Semaphore>,
    budget: Duration,
    collect: impl FnOnce(Instant) -> Value + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    // 在调度前拿 permit；超时也不释放，原生函数真正返回后才可启动下一次。
    let permit = gate.try_acquire_owned();
    tokio::spawn(async move {
        let Ok(permit) = permit else {
            emit(&logs, &context, "busy", None, None);
            return;
        };
        let worker_logs = Arc::clone(&logs);
        let worker_context = context.clone();
        let deadline = context.requested + budget;
        let worker = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            let capture_started = Instant::now();
            let started_ms = wall_ms();
            emit(
                &worker_logs,
                &worker_context,
                "started",
                Some(started_ms),
                None,
            );
            let mut result = if Instant::now() >= deadline {
                json!({"status": "unknown", "reason": "deadline_before_start"})
            } else {
                collect(deadline)
            };
            result["capture_duration_ms"] = json!(capture_started.elapsed().as_millis());
            result["deadline_exceeded"] = json!(Instant::now() >= deadline);
            emit_detected_environments(&worker_logs, &worker_context, &result);
            emit(
                &worker_logs,
                &worker_context,
                "completed",
                Some(started_ms),
                Some(result),
            );
        });
        match tokio::time::timeout_at(tokio::time::Instant::from_std(deadline), worker).await {
            Ok(Ok(())) => {}
            Ok(Err(_)) => emit(&logs, &context, "worker_error", None, None),
            Err(_) => emit(&logs, &context, "timeout", None, None),
        }
    })
}

fn emit_detected_environments(logs: &LogAggregator, context: &SnapshotContext, snapshot: &Value) {
    // 系统代理与代理 TUN 为独立环境事实；同时存在时两条都写。
    for (key, component, code, detected) in [
        (
            "system_proxy",
            "system-proxy",
            "network.environment.system-proxy",
            snapshot["system_proxy"]["detected"] == true
                || (snapshot["system_proxy"]["status"] == "ok"
                    && matches!(
                        snapshot["system_proxy"]["mode"].as_str(),
                        Some("manual" | "automatic" | "mixed")
                    )),
        ),
        (
            "proxy_tun",
            "proxy-tun",
            "network.environment.proxy-tun",
            snapshot["proxy_tun"]["status"] == "ok" && snapshot["proxy_tun"]["detected"] == true,
        ),
    ] {
        if detected {
            let mut fields = context.fields.clone();
            fields.insert("environment".into(), snapshot[key].to_string());
            let _ = logs.append_core("debug", component, code, "已识别连接时的代理环境", &fields);
        }
    }
}

fn emit(
    logs: &LogAggregator,
    context: &SnapshotContext,
    outcome: &str,
    started_ms: Option<u128>,
    snapshot: Option<Value>,
) {
    let mut fields = context.fields.clone();
    fields.insert("outcome".into(), outcome.into());
    fields.insert("observed_ms".into(), wall_ms().to_string());
    fields.insert(
        "duration_ms".into(),
        context.requested.elapsed().as_millis().to_string(),
    );
    if let Some(started) = started_ms {
        fields.insert("capture_started_ms".into(), started.to_string());
    }
    if let Some(snapshot) = snapshot {
        fields.insert("capture_finished_ms".into(), wall_ms().to_string());
        fields.insert("snapshot".into(), snapshot.to_string());
    }
    let _ = logs.append_core(
        "debug",
        "network",
        "kernel.network.snapshot",
        "连接时的网络环境快照",
        &fields,
    );
}

fn wall_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn bounded_text(text: &str) -> String {
    let mut end = text.len().min(MAX_TEXT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

fn stage(deadline: Instant, probe: impl FnOnce() -> Value) -> Value {
    if Instant::now() >= deadline {
        return json!({"status": "unknown", "reason": "capture_deadline"});
    }
    let started = Instant::now();
    let started_ms = wall_ms();
    let mut value = probe();
    value["started_ms"] = json!(started_ms);
    value["finished_ms"] = json!(wall_ms());
    value["duration_ms"] = json!(started.elapsed().as_millis());
    value
}

fn collect_network(deadline: Instant) -> Value {
    // 系统代理为独立注册表事实，先采集，避免被后续网卡/路由慢查询消耗预算。
    let system_proxy = stage(deadline, || {
        let Some(sid) = current_user_sid() else {
            return json!({"status": "unknown", "reason": "current_user_sid_unavailable"});
        };
        match system_proxy::capture_for_user(&sid) {
            Ok(raw) => proxy_raw_summary(&raw),
            Err(error) => json!({"status": "error", "native_code": error.code}),
        }
    });
    let mut luids = Vec::new();
    let mut records = Vec::new();
    let adapters = stage(deadline, || match capture_adapters() {
        Ok((snapshot, adapter_luids, adapter_records)) => {
            luids = adapter_luids;
            records = adapter_records;
            snapshot
        }
        Err(error) => error,
    });
    let proxy_tun = if adapters["status"] == "ok" {
        let detected = proxy_tun::filter_proxy_tun_adapters(&records, ENGINE_ADAPTER_NAME);
        json!({"status": "ok", "detected": !detected.is_empty(),
            "scope": "captured_adapters", "adapters_truncated": adapters["truncated"],
            "interfaces": detected.iter().map(|adapter| json!({
                "name": bounded_text(&adapter.name), "if_index": adapter.if_index,
                "kind": adapter.kind,
            })).collect::<Vec<_>>()})
    } else {
        json!({"status": "unknown", "reason": "adapter_inventory_unavailable"})
    };
    let physical = stage(deadline, || match vgdc_dns::find_physical_nics() {
        Ok(nics) => json!({"status": "ok", "selection": "vgdc_metric_order_candidates",
            "count": nics.len(), "truncated": nics.len() > MAX_INTERFACES,
            "interfaces": nics.iter().take(MAX_INTERFACES).map(|nic| json!({
                "if_index": nic.ifindex, "luid": nic.luid, "name": bounded_text(&nic.friendly_name),
                "local_ip": nic.local_ip.to_string(), "gateway": nic.gateway.to_string(),
                "ipv4_metric": nic.ipv4_metric,
            })).collect::<Vec<_>>()}),
        // 原语 String 错误不是结构化契约；不转抄未知文本。
        Err(_) => json!({"status": "error", "reason": "physical_nic_query_failed"}),
    });
    let routes = stage(deadline, || {
        let mut rows = Vec::new();
        let mut errors = Vec::new();
        let mut truncated = false;
        for luid in luids {
            if Instant::now() >= deadline || rows.len() >= MAX_ROUTES {
                truncated = true;
                break;
            }
            match routes::capture_rows(luid) {
                Ok(mut captured) => {
                    // 优先记录默认/分裂默认路由，再记录较具体前缀。
                    captured.sort_by_key(|row| row.prefix_len);
                    let remaining = MAX_ROUTES - rows.len();
                    truncated |= captured.len() > remaining;
                    rows.extend(captured.iter().take(remaining).map(|row| {
                        json!({
                            "network": row.network.to_string(), "prefix_len": row.prefix_len,
                            "next_hop": row.next_hop.to_string(), "luid": row.interface_luid,
                            "metric": row.metric, "protocol": row.protocol,
                        })
                    }));
                }
                Err(error) => errors.push(json!({"luid": luid, "native_code": error.code})),
            }
        }
        json!({"status": if adapters["status"] != "ok" { "unknown" } else if errors.is_empty() { "ok" } else { "partial" },
            "family": "ipv4", "scope": "captured_adapters", "rows": rows,
            "errors": errors, "truncated": truncated || adapters["truncated"] == true})
    });
    json!({"adapters": adapters, "physical_egress_candidates": physical,
        "proxy_tun": proxy_tun, "routes": routes, "system_proxy": system_proxy,
        "limits": {"interfaces": MAX_INTERFACES, "addresses_per_kind": MAX_ADDRESSES,
            "routes": MAX_ROUTES, "text_bytes": MAX_TEXT_BYTES}})
}

/// 周期采样只做一次接口枚举、一次关键路由读表和系统代理状态读取，不解析外网或重复物理出口探测。
fn collect_light_network(deadline: Instant) -> Value {
    let system_proxy = if let Some(sid) = current_user_sid() {
        match system_proxy::capture_for_user(&sid) {
            Ok(raw) => proxy_raw_summary(&raw),
            Err(error) => json!({"status": "error", "native_code": error.code}),
        }
    } else {
        json!({"status": "unknown", "reason": "current_user_sid_unavailable"})
    };
    let mut tunnel_luids = Vec::new();
    let adapters = if Instant::now() < deadline {
        match capture_adapters() {
            Ok((mut facts, luids, records)) => {
                let proxies = proxy_tun::filter_proxy_tun_adapters(&records, ENGINE_ADAPTER_NAME);
                tunnel_luids = records
                    .iter()
                    .zip(luids)
                    .filter_map(|(record, luid)| {
                        (record.name.to_ascii_lowercase().contains("exv")
                            || proxies
                                .iter()
                                .any(|proxy| proxy.if_index == record.if_index))
                        .then_some(luid)
                    })
                    .collect();
                if let Some(interfaces) = facts["interfaces"].as_array_mut() {
                    for interface in interfaces.iter_mut() {
                        if let Some(fields) = interface.as_object_mut() {
                            fields.remove("description");
                        }
                    }
                    interfaces.sort_by_key(|interface| interface["if_index"].as_u64());
                }
                facts
            }
            Err(error) => error,
        }
    } else {
        json!({"status": "unknown", "reason": "capture_deadline"})
    };
    let key_routes = if Instant::now() < deadline {
        capture_key_routes(&tunnel_luids)
    } else {
        json!({"status": "unknown", "reason": "capture_deadline"})
    };
    json!({"adapters": adapters, "key_routes": key_routes, "system_proxy": system_proxy})
}

fn capture_key_routes(tunnel_luids: &[u64]) -> Value {
    let mut table: *mut MIB_IPFORWARD_TABLE2 = std::ptr::null_mut();
    // SAFETY: 系统分配只读路由表，成功后由下面的 guard 释放。
    let code = unsafe { GetIpForwardTable2(AF_INET, &raw mut table) }.0;
    if code != 0 {
        return json!({"status": "error", "native_code": code});
    }
    if table.is_null() {
        return json!({"status": "ok", "family": "ipv4", "rows": [], "truncated": false});
    }
    struct TableGuard(*mut MIB_IPFORWARD_TABLE2);
    impl Drop for TableGuard {
        fn drop(&mut self) {
            // SAFETY: 指针由 GetIpForwardTable2 分配，只释放一次。
            unsafe { FreeMibTable(self.0.cast()) };
        }
    }
    let _guard = TableGuard(table);
    // SAFETY: 成功的系统表提供 NumEntries 个有效元素；限制扫描量，避免异常大表占满工作线程。
    let table = unsafe { &*table };
    let count = (table.NumEntries as usize).min(4096);
    let rows = unsafe { std::slice::from_raw_parts(table.Table.as_ptr(), count) };
    let mut facts = Vec::new();
    let mut truncated = table.NumEntries as usize > count;
    for row in rows {
        // SAFETY: AF_INET 查询结果为 IPv4，Luid 的 Value 成员由系统填充。
        let (network, next_hop, luid) = unsafe {
            (
                row.DestinationPrefix.Prefix.Ipv4.sin_addr.S_un.S_addr,
                row.NextHop.Ipv4.sin_addr.S_un.S_addr,
                row.InterfaceLuid.Value,
            )
        };
        // 默认/分裂默认、经网关路由及已识别 EXV/代理隧道接口路由。
        if row.DestinationPrefix.PrefixLength > 1 && next_hop == 0 && !tunnel_luids.contains(&luid)
        {
            continue;
        }
        if facts.len() == MAX_ROUTES {
            truncated = true;
            break;
        }
        facts.push(
            json!({"network": Ipv4Addr::from(network.to_ne_bytes()).to_string(),
            "prefix_len": row.DestinationPrefix.PrefixLength,
            "next_hop": Ipv4Addr::from(next_hop.to_ne_bytes()).to_string(),
            "luid": luid, "if_index": row.InterfaceIndex, "metric": row.Metric,
            "protocol": row.Protocol.0}),
        );
    }
    facts.sort_by_key(Value::to_string);
    json!({"status": "ok", "family": "ipv4", "scope": "default_gateway_and_identified_tunnel_routes",
        "rows": facts, "truncated": truncated})
}

fn proxy_summary(snapshot: &system_proxy::SystemProxySnapshot) -> Value {
    let mode = match snapshot.mode {
        system_proxy::SystemProxyMode::Disabled => "disabled",
        system_proxy::SystemProxyMode::Manual => "manual",
        system_proxy::SystemProxyMode::Automatic => "automatic",
        system_proxy::SystemProxyMode::Mixed => "mixed",
    };
    json!({"status": "ok", "scope": "core_current_user", "mode": mode,
        "endpoint_count": snapshot.endpoints.len(), "bypass_count": snapshot.bypass_entries.len(),
        "pac_present": snapshot.pac_url.is_some(), "auto_discovery": snapshot.auto_discovery})
}

/// 严格解析仍保留原契约；诊断同时记录各自独立的安全状态位，防止残留端点遮掉 PAC/WPAD。
fn proxy_raw_summary(raw: &system_proxy::RawInternetSettings) -> Value {
    let proxy_enable = raw.proxy_enable.as_dword();
    let auto_detect = raw.auto_detect.as_dword();
    let pac_present = match &raw.auto_config_url {
        system_proxy::RawValue::Absent => Some(false),
        value => value.as_sz().map(|url| !url.trim().is_empty()),
    };
    let detected = proxy_enable.is_some_and(|flag| flag != 0)
        || auto_detect.is_some_and(|flag| flag != 0)
        || pac_present == Some(true);
    let mut summary = match system_proxy::snapshot_from_raw(raw) {
        Ok(snapshot) => proxy_summary(&snapshot),
        Err(error) => json!({"status": "partial", "scope": "core_current_user", "mode": "unknown",
            "parse_error": {"native_code": error.code, "kind": format!("{:?}", error.kind)}}),
    };
    summary["proxy_enable"] = json!(proxy_enable);
    summary["auto_detect"] = json!(auto_detect);
    summary["pac_present"] = json!(pac_present);
    summary["detected"] = json!(detected);
    summary
}

type AdapterInventory = (Value, Vec<u64>, Vec<proxy_tun::AdapterRecord>);

fn capture_adapters() -> Result<AdapterInventory, Value> {
    let mut size = 0;
    // SAFETY: 首次只查询缓冲区尺寸，不传适配器存储。
    let code = unsafe {
        GetAdaptersAddresses(
            u32::from(AF_UNSPEC.0),
            GAA_FLAG_INCLUDE_GATEWAYS,
            None,
            None,
            &raw mut size,
        )
    };
    if code != 0 && code != 111 {
        return Err(json!({"status": "error", "native_code": code}));
    }
    if size == 0 {
        return Ok((
            json!({"status": "ok", "interfaces": [], "truncated": false}),
            Vec::new(),
            Vec::new(),
        ));
    }
    if size > MAX_ADAPTER_BUFFER_BYTES {
        return Err(
            json!({"status": "unknown", "reason": "adapter_buffer_limit", "requested_bytes": size}),
        );
    }
    let mut buffer = vec![0u64; (size as usize).div_ceil(8)];
    let pointer = buffer.as_mut_ptr().cast::<IP_ADAPTER_ADDRESSES_LH>();
    // SAFETY: u64 保证结构体对齐，尺寸至少为系统要求的 size；缓冲区覆盖以下全部读取。
    let code = unsafe {
        GetAdaptersAddresses(
            u32::from(AF_UNSPEC.0),
            GAA_FLAG_INCLUDE_GATEWAYS,
            None,
            Some(pointer),
            &raw mut size,
        )
    };
    if code != 0 {
        return Err(json!({"status": "error", "native_code": code}));
    }
    let mut current = pointer;
    let mut interfaces = Vec::new();
    let mut luids = Vec::new();
    let mut records = Vec::new();
    while !current.is_null() && interfaces.len() < MAX_INTERFACES {
        // SAFETY: 成功调用返回的系统链表，buffer 仍存活；只遍历 MAX_INTERFACES 项。
        let adapter = unsafe { &*current };
        let name = wide_text(adapter.FriendlyName.0);
        let description = wide_text(adapter.Description.0);
        // SAFETY: GetAdaptersAddresses 填充的 Luid 和 IfIndex union 成员。
        let (luid, if_index) =
            unsafe { (adapter.Luid.Value, adapter.Anonymous1.Anonymous.IfIndex) };
        let mut addresses = Vec::new();
        let mut unicast = adapter.FirstUnicastAddress;
        while !unicast.is_null() && addresses.len() < MAX_ADDRESSES {
            // SAFETY: 系统地址链表，缓冲区仍存活。
            let address = unsafe { &*unicast };
            addresses.push(json!({"ip": socket_ip(&address.Address), "prefix_len": address.OnLinkPrefixLength}));
            unicast = address.Next;
        }
        let mut dns_servers = Vec::new();
        let mut dns = adapter.FirstDnsServerAddress;
        while !dns.is_null() && dns_servers.len() < MAX_ADDRESSES {
            // SAFETY: 系统 DNS 链表，缓冲区仍存活。
            let address = unsafe { &*dns };
            dns_servers.push(socket_ip(&address.Address));
            dns = address.Next;
        }
        let mut gateways = Vec::new();
        let mut gateway = adapter.FirstGatewayAddress;
        while !gateway.is_null() && gateways.len() < MAX_ADDRESSES {
            // SAFETY: 系统网关链表，缓冲区仍存活。
            let address = unsafe { &*gateway };
            gateways.push(socket_ip(&address.Address));
            gateway = address.Next;
        }
        interfaces.push(json!({"name": name, "description": description,
            "if_index": if_index, "ipv6_if_index": adapter.Ipv6IfIndex, "luid": luid,
            "if_type": adapter.IfType, "oper_status": adapter.OperStatus.0, "mtu": adapter.Mtu,
            "ipv4_metric": adapter.Ipv4Metric, "ipv6_metric": adapter.Ipv6Metric,
            "addresses": addresses, "addresses_truncated": !unicast.is_null(),
            "dns_servers": dns_servers, "dns_truncated": !dns.is_null(),
            "gateways": gateways, "gateways_truncated": !gateway.is_null()}));
        records.push(proxy_tun::AdapterRecord {
            name,
            description,
            if_index,
        });
        luids.push(luid);
        current = adapter.Next;
    }
    Ok((
        json!({"status": "ok", "interfaces": interfaces, "truncated": !current.is_null()}),
        luids,
        records,
    ))
}

fn wide_text(pointer: *const u16) -> String {
    if pointer.is_null() {
        return String::new();
    }
    let mut units = Vec::new();
    for index in 0..MAX_TEXT_BYTES {
        // SAFETY: 来自 GetAdaptersAddresses 的 NUL 结尾字符串；遇 NUL 立即停，且限制读取长度。
        let unit = unsafe { *pointer.add(index) };
        if unit == 0 {
            break;
        }
        units.push(unit);
    }
    bounded_text(&String::from_utf16_lossy(&units))
}

fn socket_ip(address: &SOCKET_ADDRESS) -> Option<String> {
    if address.lpSockaddr.is_null() || address.iSockaddrLength < 2 {
        return None;
    }
    // SAFETY: 系统返回的 sockaddr；先检查尺寸和地址族，再访问对应结构。
    let family = unsafe { (*address.lpSockaddr).sa_family };
    if family == AF_INET
        && usize::try_from(address.iSockaddrLength).ok()? >= std::mem::size_of::<SOCKADDR_IN>()
    {
        // SAFETY: 上述大小和地址族已确认 IPv4 结构有效。
        let ip = unsafe {
            (*address.lpSockaddr.cast::<SOCKADDR_IN>())
                .sin_addr
                .S_un
                .S_addr
        };
        Some(Ipv4Addr::from(ip.to_ne_bytes()).to_string())
    } else if family == AF_INET6
        && usize::try_from(address.iSockaddrLength).ok()? >= std::mem::size_of::<SOCKADDR_IN6>()
    {
        // SAFETY: 上述大小和地址族已确认 IPv6 结构有效。
        let ip = unsafe {
            (*address.lpSockaddr.cast::<SOCKADDR_IN6>())
                .sin6_addr
                .u
                .Byte
        };
        Some(Ipv6Addr::from(ip).to_string())
    } else {
        None
    }
}
