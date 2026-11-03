// 仅实验：现役raw线程idle/cancel与独立allocation-only分列，不配置IP/路由。
use exv_engine::data_plane::{
    DataPlaneAux, EngineDataPlane, EngineDataPlaneThreads, RawPacketSnapshot,
};
use exv_vpn_win32_resource::{
    native_error::NativeError,
    proxy_tun::enumerate_adapter_records,
    wintun_adapter::{AdapterOpen, WintunAdapter},
    wintun_api::{WintunExports, WintunLibrary, FROZEN_WINTUN_DLL_SHA256},
};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use windows::{
    core::{HSTRING, PCWSTR},
    Win32::{
        Foundation::{GetLastError, HANDLE},
        NetworkManagement::{
            IpHelper::{GetIfEntry2, MIB_IF_ROW2},
            Ndis::NET_LUID_LH,
        },
    },
};

const CAPACITY: u32 = 131072;
const MAX_ALLOCS: usize = 4;

pub struct Options {
    dll: PathBuf,
    scenario: String,
    idle_ms: u64,
}
impl Options {
    pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Self, &'static str> {
        let mut args = args.into_iter();
        if args.next().as_deref() != Some("--run") {
            return Err(
                "用法：wintun_raw_local_lab --run --dll <绝对路径> --scenario idle|reserve-only [--idle-ms 100..1000]",
            );
        }
        let mut dll = None;
        let mut scenario = None;
        let mut idle_ms = None;
        while let Some(key) = args.next() {
            let value = args.next().ok_or("参数缺少取值")?;
            match key.as_str() {
                "--dll" if dll.is_none() => dll = Some(PathBuf::from(value)),
                "--scenario"
                    if scenario.is_none() && ["idle", "reserve-only"].contains(&value.as_str()) =>
                {
                    scenario = Some(value)
                }
                "--idle-ms" if idle_ms.is_none() => {
                    idle_ms = Some(
                        value
                            .parse::<u64>()
                            .ok()
                            .filter(|v| (100..=1000).contains(v))
                            .ok_or("idle-ms必须在100..1000")?,
                    )
                }
                _ => return Err("未知、重复或不支持的参数"),
            }
        }
        let dll = dll
            .filter(|p| p.is_absolute())
            .ok_or("必须显式指定DLL绝对路径")?;
        Ok(Self {
            dll,
            scenario: scenario.ok_or("必须显式选择scenario")?,
            idle_ms: idle_ms.unwrap_or(200),
        })
    }
}

fn native_error(stage: &str, error: NativeError) -> Value {
    json!({"stage":stage,"native_code":error.code,"detail":error.message})
}

fn adapters() -> Result<Value, Value> {
    enumerate_adapter_records()
        .map(|rows| {
            json!(rows
                .iter()
                .map(|row| json!({"name":row.name,"ifindex":row.if_index}))
                .collect::<Vec<_>>())
        })
        .map_err(|error| native_error("adapter_snapshot", error))
}

fn snapshot(value: RawPacketSnapshot) -> Value {
    json!({"overflow_packets":value.overflow_packets,"overflow_bytes":value.overflow_bytes,
        "full_wait_count":value.full_wait_count,"full_wait_us":value.full_wait_us,
        "empty_wait_count":value.empty_wait_count,"empty_wait_us":value.empty_wait_us})
}

struct Owner {
    library: Option<WintunLibrary>,
    adapter: Option<WintunAdapter>,
    plane: Option<EngineDataPlane>,
    threads: Option<EngineDataPlaneThreads>,
    attempted_name: Option<String>,
    identity: Option<(String, u64, u32)>,
    stages: Vec<Value>,
    finished: bool,
    cleanup: bool,
}

impl Owner {
    fn new(library: WintunLibrary) -> Self {
        Self {
            library: Some(library),
            adapter: None,
            plane: None,
            threads: None,
            attempted_name: None,
            identity: None,
            stages: Vec::new(),
            finished: false,
            cleanup: false,
        }
    }

    fn create(&mut self, name: &str) -> Result<(), Value> {
        let library = self.library.as_ref().unwrap();
        match WintunAdapter::open(library, name) {
            Err(error) if error.code == 1168 => {}
            Err(error) => return Err(native_error("name_precheck", error)),
            Ok((opened, _)) => {
                drop(opened);
                return Err(
                    json!({"stage":"name_precheck","reason":"random_name_already_present"}),
                );
            }
        }
        // wrapper可能在创建成功后查询身份失败；错误返回也必须观察本次精确名称。
        self.attempted_name = Some(name.to_owned());
        let (adapter, kind) = WintunAdapter::create(library, name, "EXV F8 Local Lab")
            .map_err(|error| native_error("create", error))?;
        self.identity = Some((
            adapter.alias().to_owned(),
            adapter.luid(),
            adapter.ifindex(),
        ));
        self.adapter = Some(adapter);
        let adapter = self.adapter.as_ref().unwrap();
        if kind != AdapterOpen::Created || !adapter.owned() || adapter.alias() != name {
            return Err(json!({"stage":"create","reason":"created_identity_mismatch"}));
        }
        self.stages.push(json!({"stage":"created","name":adapter.alias(),"luid":adapter.luid(),"ifindex":adapter.ifindex(),"ring_capacity":CAPACITY}));
        Ok(())
    }

    fn idle(&mut self, idle_ms: u64) -> Result<(), Value> {
        self.plane = Some(
            EngineDataPlane::start(
                self.library.as_ref().unwrap(),
                self.adapter.as_ref().unwrap(),
                CAPACITY,
            )
            .map_err(|error| json!({"stage":"start_session","detail":error}))?,
        );
        let (outgoing, _held_consumer) = mpsc::channel(1);
        let (_held_producer, incoming) = mpsc::channel(1);
        let failure = Arc::new(AtomicU64::new(0));
        let notified = failure.clone();
        let aux = DataPlaneAux {
            notify_data_plane_lost: Some(Arc::new(move || {
                notified.fetch_add(1, Ordering::Relaxed);
            })),
            ..Default::default()
        };
        match self
            .plane
            .as_ref()
            .unwrap()
            .spawn_raw_packet_data_plane(outgoing, incoming, None, aux)
        {
            Ok(threads) => self.threads = Some(threads),
            Err(error) => {
                self.threads = Some(error.threads);
                return Err(json!({"stage":"spawn","detail":error.message}));
            }
        }
        self.threads.as_ref().unwrap().activate();
        std::thread::sleep(Duration::from_millis(idle_ms));
        // 保持两端channel存活直到本次显式停止，避免把drop channel当作取消。
        self.stop_workers();
        let failures = failure.load(Ordering::Relaxed);
        self.stages.push(json!({"stage":"idle_completed","idle_ms":idle_ms,"failure_notifications":failures,"controlled_packets_sent":0}));
        if failures == 0 {
            Ok(())
        } else {
            Err(json!({"stage":"idle","reason":"raw_worker_failed"}))
        }
    }

    fn stop_workers(&mut self) {
        if let Some(mut threads) = self.threads.take() {
            let alive = threads.is_alive();
            let started = Instant::now();
            let (upload, download) = threads.stop_and_join();
            let elapsed_us = started.elapsed().as_micros();
            self.stages.push(json!({"stage":"workers_join_returned","alive_before_stop":alive,"stop_join_us":elapsed_us,
                "ring_upload_bytes":upload,"ring_download_bytes":download,"join_errors":null,
                "raw":threads.raw_packet_snapshot().map(snapshot)}));
            drop(threads);
        }
    }

    fn reserve_only(&mut self) -> Result<(), Value> {
        let library = self.library.as_ref().unwrap();
        let exports = *library.exports();
        // SAFETY: 已校验DLL仍存活；只读版本导出。只在已核源码的0.14驱动上执行。
        let version = unsafe { (exports.get_running_driver_version)() };
        if version != 14 {
            return Err(
                json!({"stage":"reserve_only","reason":"driver_version_not_0_14","driver_version":version}),
            );
        }
        let name = HSTRING::from(self.adapter.as_ref().unwrap().alias());
        // SAFETY: 精确本次创建者名称；第二句柄只在本函数内领用并归下面RAII所有。
        let opened = unsafe { (exports.open_adapter)(PCWSTR::from_raw(name.as_ptr())) };
        if opened.0.is_null() {
            return Err(json!({"stage":"reserve_open","native_code":unsafe {GetLastError().0}}));
        }
        let mut native = NativeReservation {
            exports,
            opened: Some(opened),
            session: None,
        };
        // SAFETY: opened有效，容量固定128KiB；无其他session/worker共享此adapter。
        let session = unsafe { (exports.start_session)(opened, CAPACITY) };
        if session.0.is_null() {
            return Err(json!({"stage":"reserve_session","native_code":unsafe {GetLastError().0}}));
        }
        native.session = Some(session);
        let started = Instant::now();
        let mut allocated = 0;
        let mut attempts = 0;
        let mut error = None;
        while attempts < MAX_ALLOCS && started.elapsed() < Duration::from_millis(100) {
            attempts += 1;
            // SAFETY: 独占活session，仅有限预约；不读写payload、不调用Send，不发布共享Tail。
            let packet = unsafe { (exports.allocate_send_packet)(session, 65535) };
            if packet.is_null() {
                error = Some(unsafe { GetLastError().0 });
                break;
            }
            allocated += 1;
        }
        // 0.14.1源码限定：EndSession关闭设备触发driver unregister/join，再整体释放ring。
        // 预约指针不在本调用后使用，不调用SendPacket向Windows栈提交任何包。
        let ending = Instant::now();
        drop(native);
        self.stages.push(json!({"stage":"reserve_only_completed","driver_version":version,"attempts":attempts,
            "allocated_packets":allocated,"reserved_payload_bytes":allocated*65535,"allocate_error":error,
            "end_session_us":ending.elapsed().as_micros(),"send_packet_calls":0,"direction":"user_to_windows_allocation_only"}));
        if error == Some(111) {
            Ok(())
        } else {
            Err(
                json!({"stage":"reserve_only","reason":"overflow_not_observed","native_code":error}),
            )
        }
    }

    fn finish(&mut self) -> bool {
        if self.finished {
            return self.cleanup;
        }
        self.stop_workers();
        drop(self.plane.take());
        self.stages.push(json!({"stage":"session_owner_released"}));
        drop(self.adapter.take());
        self.stages.push(json!({"stage":"creator_owner_released"}));
        let (cleanup, observation) = assess_cleanup(
            self.attempted_name.as_deref(),
            self.identity.as_ref(),
            |name, luid| observe_removed(self.library.as_ref().unwrap(), name, luid),
        );
        self.cleanup = cleanup;
        if let Some(observation) = observation {
            self.stages.push(observation);
        }
        drop(self.library.take());
        self.stages.push(json!({"stage":"library_released"}));
        self.finished = true;
        self.cleanup
    }
}
impl Drop for Owner {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

// 只持本次第二adapter句柄和独占session，结束顺序固定，未提交预约由EndSession整体释放。
struct NativeReservation {
    exports: WintunExports,
    opened: Option<HANDLE>,
    session: Option<HANDLE>,
}
impl Drop for NativeReservation {
    fn drop(&mut self) {
        if let Some(session) = self.session.take() {
            unsafe { (self.exports.end_session)(session) };
        }
        if let Some(opened) = self.opened.take() {
            unsafe { (self.exports.close_adapter)(opened) };
        }
    }
}

// 创建前失败没有待释放资源；尝试创建但没有身份时，名称缺失也不能代替LUID核验。
fn assess_cleanup(
    attempted_name: Option<&str>,
    identity: Option<&(String, u64, u32)>,
    mut observe: impl FnMut(&str, Option<u64>) -> Value,
) -> (bool, Option<Value>) {
    match (attempted_name, identity) {
        (_, Some((name, luid, _))) => {
            let observation = observe(name, Some(*luid));
            (observation["removed"] == true, Some(observation))
        }
        (Some(name), None) => {
            let mut observation = observe(name, None);
            observation["removed"] = json!(false);
            observation["cleanup_confirmation"] = json!("identity_unknown");
            (false, Some(observation))
        }
        (None, None) => (true, None),
    }
}

fn observe_removed(library: &WintunLibrary, name: &str, luid: Option<u64>) -> Value {
    let started = Instant::now();
    let code = luid.map(|luid| {
        let mut row = MIB_IF_ROW2 {
            InterfaceLuid: NET_LUID_LH { Value: luid },
            ..Default::default()
        };
        // SAFETY: row合法，按保存的精确LUID查询；不使用可能重用的ifindex。
        let mut code = unsafe { GetIfEntry2(&raw mut row).0 };
        while code == 0 && started.elapsed() < Duration::from_secs(2) {
            std::thread::sleep(Duration::from_millis(50));
            row = MIB_IF_ROW2 {
                InterfaceLuid: NET_LUID_LH { Value: luid },
                ..Default::default()
            };
            code = unsafe { GetIfEntry2(&raw mut row).0 };
        }
        code
    });
    let by_name = match WintunAdapter::open(library, name) {
        Ok((opened, _)) => {
            let returned = opened.luid();
            drop(opened);
            json!({"present":true,"luid":returned})
        }
        Err(error) => json!({"present":null,"native_code":error.code}),
    };
    // Win32 GetIfEntry2对不存在的LUID返回ERROR_FILE_NOT_FOUND(2)；Open精确名称返回1168。
    let removed = code.is_some_and(|code| removal_result(code, by_name["native_code"].as_u64()));
    json!({"stage":"removed_observation","name":name,"luid":luid,"get_if_entry_code":code,"by_name":by_name,"removed":removed,"elapsed_us":started.elapsed().as_micros()})
}
fn removal_result(luid_code: u32, name_code: Option<u64>) -> bool {
    luid_code == 2 && name_code == Some(1168)
}

pub fn run(options: Options, emit: &mut dyn FnMut(Value)) -> bool {
    let run_id = uuid::Uuid::new_v4().to_string();
    let started = Instant::now();
    let mut output = |mut value: Value| {
        value["schema_version"] = json!(1);
        value["run_id"] = json!(run_id);
        emit(value);
    };
    output(
        json!({"record_type":"start","mode":"local","host":"windows","scenario":options.scenario,
        "candidate":"wintun_native_local","source_revision":option_env!("EXV_LATENCY_SOURCE_REVISION"),
        "evidence_layer":"native_wintun_local","parameters":{"ring_capacity":CAPACITY,"idle_ms":options.idle_ms,"dll_sha256":FROZEN_WINTUN_DLL_SHA256,"no_ip_or_route_configuration":true}}),
    );
    let before = adapters();
    let mut owner = None;
    let result = (|| {
        before.as_ref().map_err(Clone::clone)?;
        let library =
            WintunLibrary::load(&options.dll).map_err(|error| native_error("load", error))?;
        let managed = owner.insert(Owner::new(library));
        managed.create(&format!("ExvF8Lab-{}", uuid::Uuid::new_v4().simple()))?;
        if started.elapsed() > Duration::from_secs(5) {
            return Err(json!({"stage":"setup","reason":"work_budget_exceeded"}));
        }
        match options.scenario.as_str() {
            "idle" => managed.idle(options.idle_ms),
            _ => managed.reserve_only(),
        }
    })();
    let cleanup = owner.as_mut().is_none_or(Owner::finish);
    let stages = owner
        .as_ref()
        .map(|owner| owner.stages.clone())
        .unwrap_or_default();
    let after = adapters();
    let passed = result.is_ok() && cleanup && after.is_ok();
    output(
        json!({"record_type":"final","outcome":if passed {"passed"} else {"failed"},"sent":0,"received":0,"timeouts":0,"invalid":0,"cancelled":0,"duplicates":0,"late":0,
        "elapsed_us":started.elapsed().as_micros(),"cleanup":if cleanup {"passed"} else {"failed"},
        "metrics":{"before_adapters":before.unwrap_or_else(|error|error),"after_adapters":after.unwrap_or_else(|error|error),"stages":stages,"failure":result.err(),"controlled_packets_sent":0,"native_outbound_burst":null}}),
    );
    passed
}
