use super::*;
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::{
            fs::{MetadataExt, OpenOptionsExt},
            process::{CommandExt, ExitStatusExt},
        },
    },
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    thread,
    time::Duration,
};
const BIN: &str = "/Library/sangfor/SDP/aTrust.app/Contents/Resources/bin";
const CORE_ARGS: &[&str] = &[
    "--plugin",
    "plugins/aTrustCore",
    "--enable-http",
    "--enable-event-center",
];
const DAEMON_ARGS: &[&str] = &["--plugin", "plugin-daemon", "--plugin-cmd", "|"];
const WATCHDOG_ARGS: &[&str] = &["-check", "10", "-ec", "3", "watchdog"];
const TRAYS: &[&str] = &[
    "/Applications/aTrust.app/Contents/MacOS/aTrust",
    "/Library/sangfor/SDP/aTrust.app/Contents/MacOS/aTrust",
];

pub(super) struct FileRecoveryStore {
    path: PathBuf,
}
pub(super) struct StoreLock(File);
impl Drop for StoreLock {
    fn drop(&mut self) {
        // SAFETY: the owned file descriptor remains valid for this call.
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}
impl FileRecoveryStore {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path }
    }
    fn io(&self, step: &str, error: std::io::Error) -> ReceiverError {
        ReceiverError::new(
            step,
            error.raw_os_error(),
            &format!("{}: {error}", self.path.display()),
        )
    }
    fn directory(&self) -> Result<&Path, ReceiverError> {
        self.path
            .parent()
            .ok_or_else(|| ReceiverError::new("recovery_path", None, "missing parent directory"))
    }
    fn prepare_directory(&self) -> Result<(), ReceiverError> {
        let directory = self.directory()?;
        // 共享目录沿现有归属管理；只为写操作补齐缺失路径，不修改已有属性。
        let mut builder = fs::DirBuilder::new();
        use std::os::unix::fs::DirBuilderExt;
        builder
            .recursive(true)
            .mode(0o700)
            .create(directory)
            .map_err(|error| {
                ReceiverError::new(
                    "recovery_directory",
                    error.raw_os_error(),
                    &format!("{}: {error}", directory.display()),
                )
            })
    }
    pub(super) fn cleanup_after_restore(&self) -> Result<(), ReceiverError> {
        match fs::remove_file(self.path.with_extension("lock")) {
            Ok(()) => {},
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {},
            Err(error) => return Err(self.io("remove_recovery_lock", error)),
        }
        match fs::remove_dir(self.directory()?) {
            Ok(()) => Ok(()),
            Err(error) if matches!(error.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty) => Ok(()),
            Err(error) => Err(self.io("remove_recovery_directory", error)),
        }
    }
    pub(super) fn lock(&self, deadline: Instant) -> Result<StoreLock, ReceiverError> {
        self.prepare_directory()?;
        let path = self.path.with_extension("lock");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path)
            .map_err(|e| ReceiverError::io("recovery_lock", e))?;
        check_private_file(&file, "recovery_lock")?;
        loop {
            check_deadline(deadline, "recovery_lock")?;
            // SAFETY: valid owned descriptor; flock does not outlive it.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                return Ok(StoreLock(file));
            }
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::EWOULDBLOCK) {
                return Err(ReceiverError::io("recovery_lock", error));
            }
            thread::sleep(
                Duration::from_millis(20).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
}
fn check_private_file(file: &File, step: &str) -> Result<(), ReceiverError> {
    let metadata = file.metadata().map_err(|e| ReceiverError::io(step, e))?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.mode() & 0o077 != 0
    {
        return Err(ReceiverError::new(
            step,
            Some(libc::EPERM),
            "recovery file ownership or permissions differ",
        ));
    }
    Ok(())
}
impl RecoveryStore for FileRecoveryStore {
    fn load(&self) -> Result<Option<OriginalReceiverConfig>, ReceiverError> {
        let mut file = match OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&self.path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(self.io("read_recovery", error));
            }
        };
        check_private_file(&file, "read_recovery")?;
        let mut bytes = Vec::new();
        (&mut file)
            .take(65537)
            .read_to_end(&mut bytes)
            .map_err(|e| self.io("read_recovery", e))?;
        if bytes.len() > 65536 {
            return Err(ReceiverError::new(
                "read_recovery",
                None,
                "recovery record exceeds size limit",
            ));
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| ReceiverError::new("read_recovery", None, "recovery record is invalid"))
    }
    fn save(&self, value: &OriginalReceiverConfig) -> Result<(), ReceiverError> {
        let replacing = self.load()?.is_some();
        self.prepare_directory()?;
        let temporary = self
            .path
            .with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let payload = serde_json::to_vec(value).map_err(|_| {
                ReceiverError::new(
                    "save_recovery",
                    None,
                    "cannot encode original launchd state",
                )
            })?;
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
                .open(&temporary)
                .map_err(|e| self.io("save_recovery", e))?;
            file.write_all(&payload)
                .and_then(|()| file.sync_all())
                .map_err(|e| self.io("save_recovery", e))?;
            // The operation lock serializes the first publication and later
            // target additions. Both publish only a complete, synced record.
            if replacing {
                fs::rename(&temporary, &self.path).map_err(|e| self.io("save_recovery", e))?;
            } else {
                match fs::hard_link(&temporary, &self.path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        self.load()?;
                    }
                    Err(error) => {
                        return Err(self.io("save_recovery", error));
                    }
                }
                fs::remove_file(&temporary).map_err(|e| self.io("save_recovery", e))?;
            }
            File::open(self.directory()?)
                .and_then(|f| f.sync_all())
                .map_err(|e| self.io("save_recovery", e))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
    fn remove(&self) -> Result<(), ReceiverError> {
        self.load()?;
        match fs::remove_file(&self.path) {
            Ok(()) => File::open(self.directory()?)
                .and_then(|f| f.sync_all())
                .map_err(|e| self.io("remove_recovery", e)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(self.io("remove_recovery", e)),
        }
    }
}

pub(super) struct NativeHost {
    owner_uid: u32,
}
impl NativeHost {
    pub(super) fn new(owner_uid: u32) -> Self {
        Self { owner_uid }
    }
    fn definitions(&self) -> Vec<TargetState> {
        let make = |domain: String, label: &str, program: String, args: &[&str], agents: bool| {
            TargetState {
                domain,
                label: label.into(),
                plist: format!(
                    "/Library/{}/{label}.plist",
                    if agents {
                        "LaunchAgents"
                    } else {
                        "LaunchDaemons"
                    }
                ),
                argv: std::iter::once(program)
                    .chain(args.iter().map(|s| (*s).into()))
                    .collect(),
                disabled: None,
                default_disabled: false,
                registered: false,
                running: false,
            }
        };
        let mut targets = vec![
            make(
                "system".into(),
                "com.sangfor.aTrustDaemon",
                format!("{BIN}/aTrustAgent"),
                DAEMON_ARGS,
                false,
            ),
            make(
                "system".into(),
                "com.sangfor.aTrustTunnel",
                format!("{BIN}/aTrustXtunnel"),
                &[],
                false,
            ),
            make(
                "system".into(),
                "com.sangfor.aTrustTunnelWatchDog",
                format!("{BIN}/aTrustXtunnel"),
                WATCHDOG_ARGS,
                false,
            ),
        ];
        for domain in [
            format!("gui/{}", self.owner_uid),
            format!("user/{}", self.owner_uid),
        ] {
            targets.push(make(
                domain.clone(),
                "com.sangfor.aTrustCore",
                format!("{BIN}/aTrustAgent"),
                CORE_ARGS,
                true,
            ));
            targets.push(make(
                domain,
                "com.sangfor.aTrustTray",
                TRAYS[1].into(),
                &["--", "-s", "installedstart"],
                true,
            ));
        }
        targets
    }
    fn validate_target(&self, target: &TargetState) -> Result<(), ReceiverError> {
        if self.definitions().iter().any(|known| {
            known.domain == target.domain
                && known.label == target.label
                && known.plist == target.plist
                && known.argv == target.argv
        }) {
            Ok(())
        } else {
            Err(ReceiverError::new(
                "target_identity",
                Some(libc::EPERM),
                "saved target is outside fixed official receiver identities",
            ))
        }
    }
    fn query_job(
        &self,
        target: &TargetState,
        deadline: Instant,
    ) -> Result<Option<String>, ReceiverError> {
        let output = run(
            "/bin/launchctl",
            &["print", &target.target()],
            deadline,
            "inspect_job",
        )?;
        if output.status.success() {
            return String::from_utf8(output.stdout).map(Some).map_err(|_| {
                ReceiverError::new("inspect_job", None, "launchctl output is not UTF-8")
            });
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("Could not find service") || stderr.contains("Could not find domain") {
            Ok(None)
        } else {
            Err(command_error("inspect_job", output))
        }
    }
    fn verify_job(&self, target: &TargetState, text: &str) -> Result<(), ReceiverError> {
        let lines: Vec<_> = text.lines().map(str::trim).collect();
        let program = format!("program = {}", target.argv[0]);
        let mut args = Vec::new();
        if let Some(index) = lines.iter().position(|line| *line == "arguments = {") {
            for line in &lines[index + 1..] {
                if *line == "}" {
                    break;
                }
                args.push((*line).to_owned());
            }
        }
        if !lines.contains(&program.as_str()) || args != target.argv {
            return Err(ReceiverError::new(
                "job_identity",
                Some(libc::EPERM),
                "registered job executable or arguments differ",
            ));
        }
        Ok(())
    }
    fn read_plist(
        &self,
        target: &mut TargetState,
        deadline: Instant,
    ) -> Result<bool, ReceiverError> {
        #[cfg(test)]
        let fixture = super::tests::plist_fixture();
        #[cfg(test)]
        let plist_path = fixture
            .as_deref()
            .unwrap_or_else(|| Path::new(&target.plist));
        #[cfg(not(test))]
        let plist_path = Path::new(&target.plist);
        let metadata = match fs::symlink_metadata(plist_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(ReceiverError::io("inspect_plist", error)),
        };
        #[cfg(test)]
        let expected_uid = if fixture.is_some() {
            unsafe { libc::geteuid() }
        } else {
            0
        };
        #[cfg(not(test))]
        let expected_uid = 0;
        if !metadata.is_file() || metadata.uid() != expected_uid || metadata.mode() & 0o022 != 0 {
            return Err(ReceiverError::new(
                "plist_identity",
                Some(libc::EPERM),
                "official plist ownership or permissions differ",
            ));
        }
        let text = successful(
            run(
                "/usr/bin/plutil",
                &["-convert", "json", "-o", "-", &target.plist],
                deadline,
                "inspect_plist",
            )?,
            "inspect_plist",
        )?;
        let value: serde_json::Value = serde_json::from_slice(&text)
            .map_err(|_| ReceiverError::new("inspect_plist", None, "plist JSON is invalid"))?;
        let args = value["ProgramArguments"].as_array().and_then(|a| {
            a.iter()
                .map(|v| v.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
        });
        if value["Label"].as_str() != Some(&target.label) || args.as_ref() != Some(&target.argv) {
            return Err(ReceiverError::new(
                "plist_identity",
                Some(libc::EPERM),
                "official plist label or executable arguments differ",
            ));
        }
        target.default_disabled = value["Disabled"].as_bool().unwrap_or(false);
        Ok(true)
    }
}
impl ReceiverHost for NativeHost {
    fn inspect(&self, deadline: Instant) -> Result<Observation, ReceiverError> {
        let mut targets = Vec::new();
        for mut target in self.definitions() {
            let plist = self.read_plist(&mut target, deadline)?;
            let job = self.query_job(&target, deadline)?;
            if !plist && job.is_none() {
                continue;
            }
            if let Some(job) = job {
                self.verify_job(&target, &job)?;
                target.registered = true;
                target.running = job.lines().any(|line| line.trim() == "state = running");
            }
            let output = successful(
                run(
                    "/bin/launchctl",
                    &["print-disabled", &target.domain],
                    deadline,
                    "inspect_disabled",
                )?,
                "inspect_disabled",
            )?;
            target.disabled = parse_disabled(&String::from_utf8_lossy(&output), &target.label)?;
            targets.push(target);
        }
        let processes = processes(deadline)?;
        let residual_running = processes
            .iter()
            .any(|p| is_receiver_process(p, self.owner_uid, &processes));
        Ok(Observation {
            targets,
            residual_running,
        })
    }
    fn disable(&self, target: &TargetState, deadline: Instant) -> Result<(), ReceiverError> {
        self.validate_target(target)?;
        successful(
            run(
                "/bin/launchctl",
                &["disable", &target.target()],
                deadline,
                "disable",
            )?,
            "disable",
        )?;
        Ok(())
    }
    fn bootout(&self, target: &TargetState, deadline: Instant) -> Result<(), ReceiverError> {
        self.validate_target(target)?;
        // The normal bootout command's original result is retained, even if a
        // concurrent exit makes a later observation look stopped.
        successful(
            run(
                "/bin/launchctl",
                &["bootout", &target.target()],
                deadline,
                "bootout",
            )?,
            "bootout",
        )?;
        Ok(())
    }
    fn wait_stopped(
        &self,
        targets: &[TargetState],
        deadline: Instant,
    ) -> Result<(), ReceiverError> {
        loop {
            check_deadline(deadline, "wait_launchd_stopped")?;
            let mut any = false;
            for target in targets {
                if let Some(job) = self.query_job(target, deadline)? {
                    self.verify_job(target, &job)?;
                    any = true;
                }
            }
            if !any {
                return Ok(());
            }
            thread::sleep(
                Duration::from_millis(40).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    fn cleanup(&self, deadline: Instant) -> Result<(), ReceiverError> {
        // Capture exact installation identities before signaling. Re-read each
        // PID immediately before either signal; never kill by vendor prefix.
        let initial = processes(deadline)?;
        let selected: Vec<_> = initial
            .iter()
            .filter(|p| is_receiver_process(p, self.owner_uid, &initial))
            .cloned()
            .collect();
        for process in &selected {
            signal_verified(process, libc::SIGTERM, deadline)?;
        }
        let grace = (Instant::now() + Duration::from_secs(1)).min(deadline);
        loop {
            let current = processes(deadline)?;
            let remaining: Vec<_> = current
                .iter()
                .filter(|p| is_receiver_process(p, self.owner_uid, &current))
                .collect();
            if remaining.is_empty() {
                return Ok(());
            }
            check_deadline(deadline, "cleanup_wait")?;
            if Instant::now() >= grace {
                for process in remaining {
                    signal_verified(process, libc::SIGKILL, deadline)?;
                }
            }
            thread::sleep(
                Duration::from_millis(30).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    fn restore(
        &self,
        target: &TargetState,
        deadline: Instant,
        steps: &mut Vec<String>,
    ) -> Result<(), ReceiverError> {
        self.validate_target(target)?;
        let mut current = target.clone();
        if !self.read_plist(&mut current, deadline)? {
            return Err(ReceiverError::new(
                "restore_identity",
                Some(libc::ENOENT),
                "saved official plist is absent",
            ));
        }
        let action = if target.effectively_disabled() {
            "disable"
        } else {
            "enable"
        };
        successful(
            run(
                "/bin/launchctl",
                &[action, &target.target()],
                deadline,
                "restore_disabled",
            )?,
            "restore_disabled",
        )?;
        steps.push(target.restore_step());
        // None restores effective startup through the documented enable/disable
        // API. The snapshot still records absent vs explicit false; launchd has
        // no documented per-label command that removes the override entry.
        if target.registered
            && !target.effectively_disabled()
            && self.query_job(target, deadline)?.is_none()
        {
            successful(
                run(
                    "/bin/launchctl",
                    &["bootstrap", &target.domain, &target.plist],
                    deadline,
                    "restore_bootstrap",
                )?,
                "restore_bootstrap",
            )?;
            steps.push(format!("bootstrap:{}", target.target()));
        }
        Ok(())
    }
}
pub(super) fn parse_disabled(text: &str, label: &str) -> Result<Option<bool>, ReceiverError> {
    let key = format!("\"{label}\"");
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(&key) {
            return match rest.trim().strip_prefix("=>").map(str::trim) {
                Some("true" | "disabled") => Ok(Some(true)),
                Some("false" | "enabled") => Ok(Some(false)),
                _ => Err(ReceiverError::new(
                    "inspect_disabled",
                    None,
                    "disabled override is malformed",
                )),
            };
        }
    }
    Ok(None)
}
#[derive(Clone, Debug)]
pub(super) struct ProcessRecord {
    pub(super) pid: i32,
    pub(super) uid: u32,
    pub(super) path: String,
    pub(super) args: Vec<String>,
}
pub(super) fn is_receiver_process(
    process: &ProcessRecord,
    owner_uid: u32,
    _all: &[ProcessRecord],
) -> bool {
    let args: Vec<_> = process.args.iter().map(String::as_str).collect();
    if process.path == format!("{BIN}/aTrustAgent") {
        return (process.uid == owner_uid && args == CORE_ARGS)
            || (process.uid == 0 && args == DAEMON_ARGS);
    }
    if process.path == format!("{BIN}/aTrustXtunnel") {
        return process.uid == 0 && (args.is_empty() || args == WATCHDOG_ARGS);
    }
    if process.uid != owner_uid {
        return false;
    }
    if TRAYS.contains(&process.path.as_str()) {
        return true;
    }
    for app in [
        "/Applications/aTrust.app",
        "/Library/sangfor/SDP/aTrust.app",
    ] {
        for helper in [
            "aTrust Helper",
            "aTrust Helper (GPU)",
            "aTrust Helper (Renderer)",
        ] {
            if process.path
                == format!("{app}/Contents/Frameworks/{helper}.app/Contents/MacOS/{helper}")
            {
                return true;
            }
        }
        if process.path
            == format!(
                "{app}/Contents/Frameworks/Electron Framework.framework/Helpers/chrome_crashpad_handler"
            )
        {
            return true;
        }
    }
    false
}
fn executable(pid: i32) -> Result<Option<String>, ReceiverError> {
    unsafe extern "C" {
        fn proc_pidpath(pid: i32, buffer: *mut u8, size: u32) -> i32;
    }
    let mut bytes = [0_u8; 4096];
    // SAFETY: the buffer is valid and sized exactly as passed to libproc.
    let length = unsafe { proc_pidpath(pid, bytes.as_mut_ptr(), bytes.len() as u32) };
    if length <= 0 {
        let error = std::io::Error::last_os_error();
        if matches!(error.raw_os_error(), Some(libc::ESRCH | libc::ENOENT)) {
            return Ok(None);
        }
        return Err(ReceiverError::io("process_identity", error));
    }
    let end = bytes
        .iter()
        .position(|b| *b == 0)
        .unwrap_or(length as usize);
    std::str::from_utf8(&bytes[..end])
        .map(|s| Some(s.into()))
        .map_err(|_| ReceiverError::new("process_identity", None, "process path is not UTF-8"))
}
fn processes(deadline: Instant) -> Result<Vec<ProcessRecord>, ReceiverError> {
    let output = successful(
        run(
            "/bin/ps",
            &["-axo", "pid=,ppid=,uid=,comm="],
            deadline,
            "list_processes",
        )?,
        "list_processes",
    )?;
    let mut records = Vec::new();
    for line in String::from_utf8_lossy(&output).lines() {
        let mut fields = line.split_whitespace();
        let (Some(pid), Some(_ppid), Some(uid)) = (
            fields.next().and_then(|s| s.parse::<i32>().ok()),
            fields.next().and_then(|s| s.parse::<i32>().ok()),
            fields.next().and_then(|s| s.parse::<u32>().ok()),
        ) else {
            continue;
        };
        let candidate = fields.collect::<Vec<_>>().join(" ");
        if !candidate.starts_with("/Applications/aTrust.app/")
            && !candidate.starts_with("/Library/sangfor/SDP/aTrust.app/")
        {
            continue;
        }
        let Some(path) = executable(pid)? else {
            continue;
        };
        let args = if path == format!("{BIN}/aTrustAgent") || path == format!("{BIN}/aTrustXtunnel")
        {
            let output = run(
                "/bin/ps",
                &["-ww", "-p", &pid.to_string(), "-o", "command="],
                deadline,
                "process_arguments",
            )?;
            if !output.status.success() {
                if executable(pid)?.is_none() {
                    continue;
                }
                return Err(command_error("process_arguments", output));
            }
            let command = String::from_utf8_lossy(&output.stdout);
            let Some(rest) = command.trim().strip_prefix(&path) else {
                return Err(ReceiverError::new(
                    "process_arguments",
                    None,
                    "process command differs from executable",
                ));
            };
            rest.split_whitespace().map(str::to_owned).collect()
        } else {
            Vec::new()
        };
        records.push(ProcessRecord {
            pid,
            uid,
            path,
            args,
        });
    }
    Ok(records)
}
fn signal_verified(
    process: &ProcessRecord,
    signal: i32,
    deadline: Instant,
) -> Result<(), ReceiverError> {
    check_deadline(deadline, "cleanup_signal")?;
    let current = processes(deadline)?;
    if !current.iter().any(|p| {
        p.pid == process.pid
            && p.uid == process.uid
            && p.path == process.path
            && p.args == process.args
    }) {
        return Ok(());
    }
    // SAFETY: positive PID from native inspection, exact identity rechecked.
    if unsafe { libc::kill(process.pid, signal) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ESRCH) {
            return Err(ReceiverError::io("cleanup_signal", error));
        }
    }
    Ok(())
}
fn run(
    program: &str,
    args: &[&str],
    deadline: Instant,
    step: &str,
) -> Result<Output, ReceiverError> {
    #[cfg(test)]
    if let Some(output) = super::tests::native_command_fixture(program, args, step) {
        return output;
    }
    check_deadline(deadline, step)?;
    let mut command = Command::new(program);
    command
        .env_clear()
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if unsafe { libc::geteuid() } == 0 {
        command.uid(0);
    }
    let mut child = command.spawn().map_err(|e| ReceiverError::io(step, e))?;
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    // Drain concurrently so ps and launchctl cannot block on a full pipe. Keep
    // only bounded diagnostic data while continuing to consume the streams.
    let drain = |mut pipe: Box<dyn Read + Send>| {
        thread::spawn(move || {
            let mut saved = Vec::new();
            let mut buffer = [0_u8; 8192];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let take = n.min(1_048_576_usize.saturating_sub(saved.len()));
                        saved.extend_from_slice(&buffer[..take]);
                    }
                    Err(_) => break,
                }
            }
            saved
        })
    };
    let out = drain(Box::new(stdout));
    let err = drain(Box::new(stderr));
    let mut timed_out = false;
    let result = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) => {}
            Err(error) => break Err(ReceiverError::io(step, error)),
        }
        if Instant::now() >= deadline {
            timed_out = true;
            let _ = child.kill();
            break child.wait().map_err(|e| ReceiverError::io(step, e));
        }
        thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    };
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if timed_out {
        return Err(ReceiverError::new(
            step,
            Some(libc::ETIMEDOUT),
            "native command exceeded operation deadline",
        ));
    }
    Ok(Output {
        status: result?,
        stdout,
        stderr,
    })
}
fn successful(output: Output, step: &str) -> Result<Vec<u8>, ReceiverError> {
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(command_error(step, output))
    }
}
fn command_error(step: &str, output: Output) -> ReceiverError {
    let mut error = ReceiverError::new(
        step,
        output.status.code().or_else(|| output.status.signal()),
        &String::from_utf8_lossy(&output.stderr),
    );
    error.namespace = wire::NativeErrorNamespace::Unknown;
    error
}
