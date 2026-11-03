use super::*;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

fn config(start_type: u32) -> Configuration {
    Configuration {
        start_type,
        delayed: true,
        binary_path:
            r#""C:\Program Files (x86)\Sangfor\aTrust\aTrustAgent\aTrustAgent.exe" --service"#
                .into(),
        account: "LocalSystem".into(),
    }
}
fn observed(start_type: u32, state: u32) -> Observation {
    Observation {
        config: config(start_type),
        state,
        checkpoint: 1,
        wait_hint: Duration::from_millis(10),
    }
}
struct Machine {
    current: Observation,
    events: Vec<String>,
    next: VecDeque<Observation>,
    fail_stop: bool,
    fail_kill: bool,
    fail_processes: Option<Failure>,
    processes: Vec<Process>,
}
struct Fake(Arc<Mutex<Machine>>);
impl Backend for Fake {
    fn observe(&mut self) -> Result<Option<Observation>, Failure> {
        let mut m = self.0.lock().unwrap();
        if let Some(next) = m.next.pop_front() {
            m.current = next;
        }
        Ok(Some(m.current.clone()))
    }
    fn set_start_type(&mut self, value: u32) -> Result<(), Failure> {
        let mut m = self.0.lock().unwrap();
        m.events.push(format!("start={value}"));
        m.current.config.start_type = value;
        Ok(())
    }
    fn set_delayed(&mut self, value: bool) -> Result<(), Failure> {
        let mut m = self.0.lock().unwrap();
        m.events.push(format!("delayed={value}"));
        m.current.config.delayed = value;
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Failure> {
        let mut m = self.0.lock().unwrap();
        m.events.push("stop".into());
        if m.fail_stop {
            return Err(Failure::new("stop", 5, "Access is denied"));
        }
        m.current.state = 1;
        Ok(())
    }
    fn processes(&mut self) -> Result<Vec<Process>, Failure> {
        let mut m = self.0.lock().unwrap();
        if let Some(error) = m.fail_processes.take() {
            return Err(error);
        }
        Ok(m.processes.clone())
    }
    fn terminate(&mut self, process: &Process, _: Instant) -> Result<(), Failure> {
        let pid = process.pid;
        let mut m = self.0.lock().unwrap();
        m.events.push(format!("kill={pid}"));
        if m.fail_kill {
            return Err(Failure::new("cleanup", 87, "Invalid parameter"));
        }
        m.processes.retain(|p| p.pid != pid);
        Ok(())
    }
    fn pause(&mut self, _: Duration) {
        let mut m = self.0.lock().unwrap();
        m.events.push("wait".into());
        if m.current.state == 3 {
            m.current.state = 1;
            m.current.checkpoint += 1;
        }
    }
}
fn fixture(
    start_type: u32,
    state: u32,
) -> (tempfile::TempDir, Arc<Mutex<Machine>>, OfficialAuthReceiver) {
    let dir = tempfile::tempdir().unwrap();
    let machine = Arc::new(Mutex::new(Machine {
        current: observed(start_type, state),
        events: vec![],
        next: VecDeque::new(),
        fail_stop: false,
        fail_kill: false,
        fail_processes: None,
        processes: vec![],
    }));
    let controller = OfficialAuthReceiver {
        state: Mutex::new(State {
            backend: Box::new(Fake(machine.clone())),
            store: Box::new(FileStore(dir.path().join("official-auth-receiver.json"))),
        }),
    };
    (dir, machine, controller)
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(2)
}
#[test]
fn official_auth_receiver_stopped_but_startable_is_reported() {
    let (_dir, _m, c) = fixture(2, 1);
    let result = c.execute(1, deadline());
    assert_eq!(result.current.unwrap().config.start_type, 2);
}
#[test]
fn official_auth_receiver_save_disable_stop_cleanup_order() {
    let (dir, m, c) = fixture(2, 4);
    m.lock().unwrap().processes = vec![
        Process {
            pid: 12,
            path: PathBuf::from(r"C:\Program Files (x86)\Sangfor\aTrust\aTrustTray\aTrustTray.exe"),
        },
        Process {
            pid: 99,
            path: PathBuf::from(r"C:\Other\aTrustTray.exe"),
        },
    ];
    let result = c.execute(2, deadline());
    assert!(result.error.is_none(), "{:?}", result.error);
    let saved: Configuration = serde_json::from_slice(
        &std::fs::read(dir.path().join("official-auth-receiver.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(saved, config(2));
    assert_eq!(m.lock().unwrap().events, vec!["start=4", "stop", "kill=12"]);
    assert_eq!(
        result.completed_steps,
        vec![
            "save_original_config",
            "disable_service",
            "stop_service",
            "cleanup_processes"
        ]
    );
}
#[test]
fn official_auth_receiver_repeat_and_restart_keep_first_snapshot() {
    let (dir, m, c) = fixture(2, 4);
    c.execute(2, deadline());
    let second = c.execute(2, deadline());
    assert_eq!(second.original.unwrap(), config(2));
    let restarted = OfficialAuthReceiver {
        state: Mutex::new(State {
            backend: Box::new(Fake(m)),
            store: Box::new(FileStore(dir.path().join("official-auth-receiver.json"))),
        }),
    };
    assert_eq!(
        restarted.execute(4, deadline()).original.unwrap(),
        config(2)
    );
}
#[test]
fn official_auth_receiver_stop_failure_retains_disabled_and_recovery() {
    let (_dir, m, c) = fixture(2, 4);
    m.lock().unwrap().fail_stop = true;
    let result = c.execute(2, deadline());
    assert_eq!(result.error.unwrap().code, 5);
    assert_eq!(result.current.unwrap().config.start_type, 4);
    assert_eq!(result.original.unwrap(), config(2));
    assert_eq!(
        result.completed_steps,
        vec!["save_original_config", "disable_service"]
    );
}
#[test]
fn official_auth_receiver_restore_original_disabled_and_delayed() {
    let (dir, m, c) = fixture(4, 4);
    c.execute(2, deadline());
    m.lock().unwrap().current.config.delayed = false;
    let result = c.execute(3, deadline());
    assert!(result.error.is_none());
    let machine = m.lock().unwrap();
    assert_eq!(machine.current.config, config(4));
    assert_eq!(machine.current.state, 1);
    assert!(!dir.path().join("official-auth-receiver.json").exists());
}
#[test]
fn official_auth_receiver_expired_deadline_performs_no_io() {
    let (dir, m, c) = fixture(2, 4);
    let result = c.execute(2, Instant::now() - Duration::from_secs(1));
    assert_eq!(result.error.unwrap().code, 1460);
    assert!(m.lock().unwrap().events.is_empty());
    assert!(!dir.path().join("official-auth-receiver.json").exists());
}
#[test]
fn official_auth_receiver_stop_pending_waits_before_cleanup() {
    let (_dir, m, c) = fixture(2, 3);
    let result = c.execute(2, deadline());
    assert!(result.error.is_none());
    assert_eq!(m.lock().unwrap().events, vec!["start=4", "wait"]);
    assert_eq!(result.current.unwrap().state, 1);
}
#[test]
fn official_auth_receiver_cleanup_failure_preserves_original_code_and_steps() {
    let (_dir, m, c) = fixture(2, 4);
    {
        let mut m = m.lock().unwrap();
        m.fail_kill = true;
        m.processes = vec![Process {
            pid: 12,
            path: PathBuf::from(
                r"C:\Program Files (x86)\Sangfor\aTrust\aTrustAgent\aTrustAgent.exe",
            ),
        }];
    }
    let result = c.execute(2, deadline());
    assert_eq!(result.error.unwrap().code, 87);
    assert_eq!(
        result.completed_steps,
        vec!["save_original_config", "disable_service", "stop_service"]
    );
    assert_eq!(result.original.unwrap(), config(2));
}
#[test]
fn official_auth_receiver_restoring_reinstalled_service_reports_identity_change() {
    let (_dir, m, c) = fixture(2, 4);
    c.execute(2, deadline());
    m.lock().unwrap().current.config.binary_path = r"C:\NewInstall\aTrustAgent.exe".into();
    let result = c.execute(3, deadline());
    assert_eq!(result.error.unwrap().step, "installation_identity");
    assert_eq!(m.lock().unwrap().current.config.start_type, 4);
    assert!(result.original.is_some());
}

#[test]
fn official_auth_receiver_recovery_path_failure_does_not_disable() {
    let (dir, m, _) = fixture(2, 4);
    let parent = dir.path().join("file");
    std::fs::write(&parent, b"occupied").unwrap();
    let c = OfficialAuthReceiver {
        state: Mutex::new(State {
            backend: Box::new(Fake(m.clone())),
            store: Box::new(FileStore(parent.join("recovery.json"))),
        }),
    };
    let result = c.execute(2, deadline());
    assert_eq!(result.error.unwrap().step, "save_original_config");
    assert!(result.completed_steps.is_empty());
    assert!(!result.recovery_available);
    assert!(result.original.is_none());
    assert!(m.lock().unwrap().events.is_empty());
    assert_eq!(m.lock().unwrap().current.config.start_type, 2);
}
#[test]
fn official_auth_receiver_queued_restart_is_disabled_even_when_stopped() {
    let (_dir, m, c) = fixture(2, 1);
    let result = c.execute(2, deadline());
    assert!(result.error.is_none());
    assert_eq!(m.lock().unwrap().current.config.start_type, 4);
    assert_eq!(m.lock().unwrap().events, vec!["start=4"]);
}

struct SaveFailure(FileStore);
impl Store for SaveFailure {
    fn load(&self) -> Result<Option<Configuration>, Failure> {
        self.0.load()
    }
    fn save_first(&self, _: &Configuration) -> Result<(), Failure> {
        Err(Failure::new(
            "save_original_config",
            112,
            "There is not enough space on the disk",
        ))
    }
    fn clear(&self) -> Result<(), Failure> {
        self.0.clear()
    }
}
#[test]
fn official_auth_receiver_snapshot_save_failure_leaves_service_unchanged() {
    let (dir, m, _) = fixture(2, 4);
    let c = OfficialAuthReceiver {
        state: Mutex::new(State {
            backend: Box::new(Fake(m.clone())),
            store: Box::new(SaveFailure(FileStore(
                dir.path().join("official-auth-receiver.json"),
            ))),
        }),
    };
    let result = c.execute(2, deadline());
    assert_eq!(result.error.unwrap().code, 112);
    assert!(result.completed_steps.is_empty());
    assert!(!result.recovery_available);
    assert_eq!(m.lock().unwrap().current.config.start_type, 2);
    assert!(m.lock().unwrap().events.is_empty());
}
#[test]
fn official_auth_receiver_wire_reports_source_startability_and_partial_error() {
    let (_dir, m, c) = fixture(2, 1);
    let status = c.control(1, deadline(), &[9; 16]).receiver.unwrap();
    assert!(!status.running);
    assert!(status.can_start);
    assert!(!status.recovery_available);
    assert_eq!(
        status.current_config.unwrap().windows_delayed_auto_start,
        Some(true)
    );
    assert_eq!(
        status.source,
        r"aTrustService; image=C:\Program Files (x86)\Sangfor\aTrust\aTrustAgent\aTrustAgent.exe; account=LocalSystem"
    );
    {
        let mut m = m.lock().unwrap();
        m.current.state = 4;
        m.fail_stop = true;
    }
    let reply = c.control(2, deadline(), &[9; 16]);
    let status = reply.receiver.unwrap();
    assert!(status.running);
    assert!(!status.can_start);
    assert!(status.recovery_available);
    assert_eq!(status.original_config.unwrap().windows_start_type, Some(2));
    assert_eq!(status.current_config.unwrap().windows_start_type, Some(4));
    let error = reply.error.unwrap();
    assert_eq!(error.native.unwrap().code, 5);
    assert!(error.operation_detail.unwrap().contains("Access is denied"));
}
#[test]
fn official_auth_receiver_restore_reply_preserves_display_original_and_clears_recovery() {
    let (_dir, _m, c) = fixture(2, 4);
    c.execute(2, deadline());
    let reply = c.control(3, deadline(), &[9; 16]);
    assert!(reply.error.is_none());
    let status = reply.receiver.unwrap();
    assert!(!status.recovery_available);
    assert!(status.can_start);
    assert!(!status.running);
    assert_eq!(status.original_config.unwrap().windows_start_type, Some(2));
    assert_eq!(status.current_config.unwrap().windows_start_type, Some(2));
    assert_eq!(
        reply.completed_steps,
        vec![
            "restore_start_type",
            "restore_delayed_auto_start",
            "clear_recovery"
        ]
    );
}

fn installed_processes() -> Vec<Process> {
    [
        (
            12,
            r"C:\Program Files (x86)\Sangfor\aTrust\aTrustTray\aTrustTray.exe",
        ),
        (
            13,
            r"C:\Program Files (x86)\Sangfor\aTrust\aTrustAgent\aTrustAgent.exe",
        ),
        (
            14,
            r"C:\Program Files (x86)\Sangfor\aTrust\aTrustAgent\xtunnel-32\aTrustXtunnel.exe",
        ),
        (
            15,
            r"C:\Program Files (x86)\Sangfor\aTrust\aTrustAgent\xtunnel-64\aTrustXtunnel.exe",
        ),
        (98, r"C:\Other\aTrustTray\aTrustTray.exe"),
        (
            99,
            r"C:\Program Files (x86)\Sangfor\aTrust-other\aTrustTray\aTrustTray.exe",
        ),
    ]
    .into_iter()
    .map(|(pid, path)| Process {
        pid,
        path: PathBuf::from(path),
    })
    .collect()
}
#[test]
fn official_auth_receiver_inspect_reports_each_nested_component() {
    for process in installed_processes().into_iter().take(4) {
        let (_dir, m, c) = fixture(4, STOPPED);
        let pid = process.pid;
        m.lock().unwrap().processes = vec![process];
        let reply = c.control(1, deadline(), &[9; 16]);
        assert!(reply.error.is_none(), "pid={pid}: {:?}", reply.error);
        assert!(reply.receiver.unwrap().running, "installed pid={pid}");
    }
}
#[test]
fn official_auth_receiver_cleanup_includes_nested_components_and_preserves_other_installations() {
    let (_dir, m, c) = fixture(2, 4);
    m.lock().unwrap().processes = installed_processes();
    let result = c.execute(2, deadline());
    assert!(result.error.is_none(), "{:?}", result.error);
    let machine = m.lock().unwrap();
    assert_eq!(
        machine.events,
        vec![
            "start=4", "stop", "kill=12", "kill=13", "kill=14", "kill=15"
        ]
    );
    assert_eq!(
        machine.processes.iter().map(|p| p.pid).collect::<Vec<_>>(),
        vec![98, 99]
    );
    assert!(!result.processes_running);
    drop(machine);
    assert!(!c.control(1, deadline(), &[9; 16]).receiver.unwrap().running);
}

struct SavedThenFailed(FileStore);
impl Store for SavedThenFailed {
    fn load(&self) -> Result<Option<Configuration>, Failure> {
        self.0.load()
    }
    fn save_first(&self, config: &Configuration) -> Result<(), Failure> {
        self.0.save_first(config)?;
        Err(Failure::new(
            "save_original_config",
            5,
            "flush directory: Access is denied",
        ))
    }
    fn clear(&self) -> Result<(), Failure> {
        self.0.clear()
    }
}
#[test]
fn official_auth_receiver_save_error_reports_existing_snapshot_and_retry_preserves_it() {
    let (dir, m, _) = fixture(2, STOPPED);
    m.lock().unwrap().processes = vec![installed_processes().remove(0)];
    let path = dir.path().join("official-auth-receiver.json");
    let c = OfficialAuthReceiver {
        state: Mutex::new(State {
            backend: Box::new(Fake(m.clone())),
            store: Box::new(SavedThenFailed(FileStore(path.clone()))),
        }),
    };
    let reply = c.control(2, deadline(), &[9; 16]);
    let saved: Configuration = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved, config(2));
    let error = reply.error.unwrap();
    assert_eq!(error.native.unwrap().code, 5);
    assert_eq!(error.reason, "platform/auth_receiver_save_original_config");
    assert!(
        error
            .operation_detail
            .unwrap()
            .contains("flush directory: Access is denied")
    );
    assert!(reply.completed_steps.is_empty());
    let status = reply.receiver.unwrap();
    assert!(status.recovery_available);
    assert_eq!(status.original_config.unwrap().windows_start_type, Some(2));
    assert!(status.running);
    assert_eq!(m.lock().unwrap().current.config.start_type, 2);
    assert!(m.lock().unwrap().events.is_empty());
    m.lock().unwrap().current.config.start_type = 3;
    let retry = c.execute(2, deadline());
    assert!(retry.error.is_none(), "{:?}", retry.error);
    assert_eq!(retry.original.unwrap(), config(2));
    assert_eq!(
        serde_json::from_slice::<Configuration>(&std::fs::read(path).unwrap()).unwrap(),
        saved
    );
}

struct ReadFailure;
impl Store for ReadFailure {
    fn load(&self) -> Result<Option<Configuration>, Failure> {
        Err(Failure::new(
            "read_recovery",
            23,
            "Data error (cyclic redundancy check)",
        ))
    }
    fn save_first(&self, _: &Configuration) -> Result<(), Failure> {
        panic!("read failure must precede snapshot writes")
    }
    fn clear(&self) -> Result<(), Failure> {
        panic!("read failure must precede recovery removal")
    }
}
#[test]
fn official_auth_receiver_read_failure_preserves_code_and_service_configuration() {
    let (_dir, m, _) = fixture(2, 4);
    let c = OfficialAuthReceiver {
        state: Mutex::new(State {
            backend: Box::new(Fake(m.clone())),
            store: Box::new(ReadFailure),
        }),
    };
    let result = c.execute(2, deadline());
    assert_eq!(
        result.error,
        Some(Failure::new(
            "read_recovery",
            23,
            "Data error (cyclic redundancy check)"
        ))
    );
    assert!(result.completed_steps.is_empty());
    assert!(!result.recovery_available);
    assert_eq!(m.lock().unwrap().current.config, config(2));
    assert!(m.lock().unwrap().events.is_empty());
}
#[test]
fn official_auth_receiver_restore_after_cleanup_failure_reports_remaining_process() {
    let (dir, m, c) = fixture(2, 4);
    {
        let mut machine = m.lock().unwrap();
        machine.fail_kill = true;
        machine.processes = vec![installed_processes().remove(1)];
    }
    assert_eq!(c.execute(2, deadline()).error.unwrap().code, 87);
    let reply = c.control(3, deadline(), &[9; 16]);
    assert!(reply.error.is_none(), "{:?}", reply.error);
    let status = reply.receiver.unwrap();
    assert!(status.running);
    assert!(status.can_start);
    assert!(!status.recovery_available);
    assert_eq!(status.current_config.unwrap().windows_start_type, Some(2));
    assert_eq!(status.original_config.unwrap().windows_start_type, Some(2));
    let machine = m.lock().unwrap();
    assert_eq!(machine.current.state, STOPPED);
    assert_eq!(machine.processes.len(), 1);
    assert_eq!(machine.current.config, config(2));
    assert!(!dir.path().join("official-auth-receiver.json").exists());
}

#[test]
fn official_auth_receiver_live_inspection_failure_keeps_first_error_after_reobservation() {
    let (dir, machine, controller) = fixture(2, 4);
    let original = Failure::new(
        "inspect_processes",
        31,
        "QueryFullProcessImageNameW pid=42 name=aTrustAgent.exe phase=enumeration; original error",
    );
    machine.lock().unwrap().fail_processes = Some(original.clone());
    let result = controller.execute(2, deadline());
    assert_eq!(result.error, Some(original));
    assert_eq!(
        result.completed_steps,
        vec!["save_original_config", "disable_service", "stop_service"]
    );
    assert_eq!(result.original, Some(config(2)));
    assert_eq!(result.current.as_ref().unwrap().state, STOPPED);
    assert_eq!(result.current.as_ref().unwrap().config.start_type, DISABLED);
    assert!(result.recovery_available);
    assert!(dir.path().join("official-auth-receiver.json").exists());
    assert!(machine.lock().unwrap().fail_processes.is_none());
}
