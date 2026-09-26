//! Darwin 连接诊断：只读本机事实，异步限频采样，不参与连接或重连决策。
use crate::{log_aggregator::LogAggregator, proxy_tun::ProxyTunCache};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Semaphore;

const PERIOD: Duration = Duration::from_secs(5);
const BUDGET: Duration = Duration::from_secs(3);
static GATE: OnceLock<Arc<Semaphore>> = OnceLock::new();

#[derive(Clone)]
pub(crate) struct Context {
    operation_id: String,
    generation: u64,
    attempt: u32,
}
impl Context {
    pub(crate) fn new(operation_id: &[u8], generation: u64, attempt: u32) -> Self {
        Self {
            operation_id: operation_id
                .iter()
                .take(16)
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            generation,
            attempt,
        }
    }
    fn fields(&self) -> Vec<(&'static str, String)> {
        vec![
            ("operation_id", self.operation_id.clone()),
            ("connection_generation", self.generation.to_string()),
            ("reconnect_attempt", self.attempt.to_string()),
            ("core_pid", std::process::id().to_string()),
            (
                "build_identity",
                serde_json::to_string(&crate::build_identity::fields()).unwrap_or_default(),
            ),
        ]
    }
}
#[derive(Clone)]
struct Sample {
    facts: Value,
    finished: Instant,
    finished_ms: u128,
    duration_ms: u128,
}
fn wall_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

// permit 由原生 worker 持有：超时不能杀死系统调用，但也不会释放并发槽叠加工作。
async fn sample(cache: Arc<ProxyTunCache>) -> Result<Sample, &'static str> {
    capture(
        Arc::clone(GATE.get_or_init(|| Arc::new(Semaphore::new(1)))),
        BUDGET,
        move || collect(&cache),
    )
    .await
}
async fn capture(
    gate: Arc<Semaphore>,
    budget: Duration,
    collect: impl FnOnce() -> Value + Send + 'static,
) -> Result<Sample, &'static str> {
    let permit = gate.try_acquire_owned().map_err(|_| "busy")?;
    let worker = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        let started = Instant::now();
        let facts = collect();
        Sample {
            facts,
            finished: Instant::now(),
            finished_ms: wall_ms(),
            duration_ms: started.elapsed().as_millis(),
        }
    });
    tokio::time::timeout(budget, worker)
        .await
        .map_err(|_| "timeout")?
        .map_err(|_| "worker_error")
}
fn collect(cache: &ProxyTunCache) -> Value {
    // 只记录协议开关/模式，绝不取代理 URL、PAC 内容、用户名或连接配置。
    let tun = cache.current();
    let proxy = cache.system_proxy();
    let interfaces = match crate::proxy_tun::collect_interface_facts() {
        Ok(mut values) => {
            values.sort_by_key(|value| value.if_index);
            let truncated = values.len() > 32;
            json!({"status":"ok", "truncated":truncated, "ipv4_interfaces":values.iter().take(32).map(|value| {
                let mut addresses = value.ipv4_addresses.clone(); addresses.sort();
                json!({"index":value.if_index,"name":value.name.chars().take(32).collect::<String>(),
                    "addresses":addresses.iter().take(8).map(ToString::to_string).collect::<Vec<_>>(), "addresses_truncated":addresses.len()>8})
            }).collect::<Vec<_>>()})
        }
        Err(error) => json!({"status":"read_failed", "os_error":error.raw_os_error()}),
    };
    let routes = match crate::proxy_tun::collect_route_facts_bounded(4 * 1024 * 1024) {
        Ok(mut values) => {
            // 默认路由、半默认路由及 Fake-IP 网段；不是完整路由表。
            values.retain(|value| {
                value.prefix_len <= 1 || crate::proxy_tun::is_fake_ip_address(value.destination)
            });
            values.sort_by_key(|value| (value.destination, value.prefix_len, value.if_index));
            json!({"status":"ok", "family":"ipv4", "truncated":values.len()>128,
                "entries":values.iter().take(128).map(|value| json!({"destination":value.destination.to_string(),"prefix_len":value.prefix_len,"interface_index":value.if_index})).collect::<Vec<_>>()})
        }
        Err(error) => json!({"status":"read_failed", "os_error":error.raw_os_error()}),
    };
    json!({"interfaces":interfaces,"key_routes":routes,
        "primary_interfaces":crate::system_proxy::primary_network_interfaces().into_iter().take(2).map(|(name,ipv6)|json!({"name":name,"ipv6":ipv6})).collect::<Vec<_>>(),
        "system_proxy":proxy.map(|value|json!({"mode":value.mode})),
        "proxy_tun":tun.map(|value|json!({"detected":value.detected}))})
}
fn emit_sample(
    logs: &LogAggregator,
    context: &Context,
    trigger: &str,
    sample: &Sample,
    previous: Option<&Sample>,
) {
    let mut fields = context.fields();
    fields.extend([
        ("trigger", trigger.into()),
        ("sample_finished_ms", sample.finished_ms.to_string()),
        ("capture_duration_ms", sample.duration_ms.to_string()),
        ("snapshot", sample.facts.to_string()),
    ]);
    let code = if let Some(previous) = previous {
        fields.push(("previous_snapshot", previous.facts.to_string()));
        fields.push((
            "previous_sample_finished_ms",
            previous.finished_ms.to_string(),
        ));
        "kernel.network.environment_changed"
    } else {
        "kernel.network.snapshot"
    };
    logs.append_core(
        "debug",
        "network",
        code,
        "本机网络环境已采样",
        &fields,
    );
}
pub(crate) fn request_snapshot(
    logs: Arc<LogAggregator>,
    cache: Arc<ProxyTunCache>,
    context: Context,
    trigger: &'static str,
) {
    if !cache.is_production() {
        return;
    }
    tokio::spawn(async move {
        match sample(cache).await {
            Ok(sample) => emit_sample(&logs, &context, trigger, &sample, None),
            Err(outcome) => {
                let mut fields = context.fields();
                fields.extend([("trigger", trigger.into()), ("outcome", outcome.into())]);
                logs.append_core(
                    "debug",
                    "network",
                    "kernel.network.snapshot_skipped",
                    "网络诊断未完成，业务流程继续",
                    &fields,
                );
            }
        }
    });
}

#[derive(Default)]
pub(crate) struct NetworkMonitor {
    task: Option<tokio::task::JoinHandle<()>>,
    latest: Arc<Mutex<Option<Sample>>>,
}
impl Drop for NetworkMonitor {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl NetworkMonitor {
    pub(crate) fn start(
        &mut self,
        logs: Arc<LogAggregator>,
        cache: Arc<ProxyTunCache>,
        context: Context,
    ) {
        if self.task.is_some() || !cache.is_production() {
            return;
        }
        let latest = Arc::clone(&self.latest);
        self.task = Some(tokio::spawn(async move {
            let mut previous_outcome = None;
            loop {
                match sample(Arc::clone(&cache)).await {
                    Ok(sample) => {
                        previous_outcome = None;
                        commit_sample(&latest, &logs, &context, sample);
                    }
                    Err(outcome) if previous_outcome != Some(outcome) => {
                        previous_outcome = Some(outcome);
                        let mut fields = context.fields();
                        fields.push(("outcome", outcome.into()));
                        logs.append_core(
                            "debug",
                            "network",
                            "kernel.network.monitor_skipped",
                            "本轮环境观测未完成，保留此前缓存",
                            &fields,
                        );
                    }
                    Err(_) => {}
                }
                tokio::time::sleep(PERIOD).await;
            }
        }));
    }
    pub(crate) fn record_and_stop(
        &mut self,
        logs: &LogAggregator,
        context: &Context,
        trigger: &str,
    ) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let mut fields = context.fields();
        fields.push(("trigger", trigger.into()));
        let latest = self
            .latest
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(sample) = latest.as_ref() {
            fields.extend([
                ("snapshot", sample.facts.to_string()),
                ("sample_finished_ms", sample.finished_ms.to_string()),
                (
                    "cache_age_ms",
                    sample.finished.elapsed().as_millis().to_string(),
                ),
            ]);
        } else {
            fields.push(("cache_status", "no_sample".into()));
        }
        logs.append_core(
            "debug",
            "network",
            "kernel.network.cached_before_failure",
            "沿用断开前的网络快照",
            &fields,
        );
    }
}

fn commit_sample(
    latest: &Mutex<Option<Sample>>,
    logs: &LogAggregator,
    context: &Context,
    sample: Sample,
) {
    let mut latest = latest.lock().unwrap_or_else(|error| error.into_inner());
    if latest
        .as_ref()
        .is_none_or(|prior| prior.facts != sample.facts)
    {
        emit_sample(logs, context, "connected_monitor", &sample, latest.as_ref());
    }
    *latest = Some(sample);
}
