//! 固定 aTrustService 的 SCM 与认证残余进程原生适配。
use super::*;
use windows::{
    Win32::{
        Foundation::{CloseHandle, HANDLE, WAIT_FAILED, WAIT_OBJECT_0},
        System::{
            Diagnostics::ToolHelp::{
                CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW,
                TH32CS_SNAPPROCESS,
            },
            Services::*,
            Threading::{
                OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE, PROCESS_TERMINATE, QueryFullProcessImageNameW,
                TerminateProcess, WaitForSingleObject,
            },
        },
    },
    core::{PCWSTR, PWSTR},
};

pub(super) struct NativeBackend;
struct ServiceHandle(SC_HANDLE);
impl Drop for ServiceHandle {
    fn drop(&mut self) {
        // SAFETY: 此句柄只由成功的 SCM open 创建，此处关闭一次。
        unsafe {
            let _ = CloseServiceHandle(self.0);
        }
    }
}
struct ProcessHandle(HANDLE);
impl Drop for ProcessHandle {
    fn drop(&mut self) {
        // SAFETY: 此句柄由成功的 snapshot/OpenProcess 创建，此处关闭一次。
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
fn native_error(step: &'static str, error: windows::core::Error) -> Failure {
    let code = error.code().0 as u32;
    let code = if code & 0xffff_0000 == 0x8007_0000 {
        code & 0xffff
    } else {
        code
    };
    Failure::new(step, code, error.to_string())
}
fn open_service(access: u32, step: &'static str) -> Result<ServiceHandle, Failure> {
    // SAFETY: NULL machine/database selects local SCM, every successful open is owned.
    let manager = ServiceHandle(
        unsafe { OpenSCManagerW(PCWSTR::null(), PCWSTR::null(), SC_MANAGER_CONNECT) }
            .map_err(|e| native_error(step, e))?,
    );
    let name: Vec<u16> = "aTrustService".encode_utf16().chain(Some(0)).collect();
    // SAFETY: NUL-terminated constant service name remains alive for the call.
    unsafe { OpenServiceW(manager.0, PCWSTR(name.as_ptr()), access) }
        .map(ServiceHandle)
        .map_err(|e| native_error(step, e))
}
fn process_error(
    step: &'static str,
    api: &str,
    context: &str,
    error: windows::core::Error,
) -> Failure {
    let mut failure = native_error(step, error);
    failure.detail = format!(
        "{api} {context}; Win32 {}: {}",
        failure.code, failure.detail
    );
    tracing::warn!(step, code = failure.code, detail = %failure.detail, "认证残余进程操作失败");
    failure
}

fn accept_exited_process(
    mut error: Failure,
    exited: impl FnOnce() -> Result<bool, Failure>,
) -> Result<(), Failure> {
    match exited() {
        Ok(true) => Ok(()),
        Ok(false) => Err(error),
        Err(wait_error) => {
            error
                .detail
                .push_str(&format!("; 退出状态读取失败：{}", wait_error.detail));
            Err(error)
        }
    }
}

fn process_exited(handle: HANDLE, step: &'static str, context: &str) -> Result<bool, Failure> {
    // SAFETY: 调用方持有同一进程对象的 SYNCHRONIZE 句柄，只在实际操作失败后读取退出事实。
    let result = unsafe { WaitForSingleObject(handle, 0) };
    if result == WAIT_FAILED {
        Err(process_error(
            step,
            "WaitForSingleObject",
            context,
            windows::core::Error::from_thread(),
        ))
    } else {
        Ok(result == WAIT_OBJECT_0)
    }
}

fn image_path(
    handle: HANDLE,
    pid: u32,
    name: &str,
    phase: &str,
) -> Result<Option<PathBuf>, Failure> {
    let mut buffer = vec![0_u16; 32768];
    let mut size = buffer.len() as u32;
    // SAFETY: handle is open, buffer is writable for the supplied capacity.
    let query = unsafe {
        QueryFullProcessImageNameW(
            handle,
            PROCESS_NAME_WIN32,
            PWSTR(buffer.as_mut_ptr()),
            &raw mut size,
        )
    };
    if let Err(error) = query {
        let context = format!("pid={pid} name={name} phase={phase}");
        let error = process_error(
            "inspect_processes",
            "QueryFullProcessImageNameW",
            &context,
            error,
        );
        accept_exited_process(error, || {
            process_exited(handle, "inspect_processes", &context)
        })?;
        return Ok(None);
    }
    Ok(Some(PathBuf::from(String::from_utf16_lossy(
        &buffer[..size as usize],
    ))))
}
impl Backend for NativeBackend {
    fn observe(&mut self) -> Result<Option<Observation>, Failure> {
        let service = match open_service(
            SERVICE_QUERY_CONFIG | SERVICE_QUERY_STATUS,
            "inspect_service",
        ) {
            Ok(service) => service,
            Err(error) if error.code == 1060 => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut needed = 0;
        // SAFETY: size probe uses a NULL buffer and a valid output size pointer.
        if let Err(error) = unsafe { QueryServiceConfigW(service.0, None, 0, &raw mut needed) } {
            let error = native_error("inspect_service", error);
            if error.code != 122 {
                return Err(error);
            }
        }
        let words = (needed as usize)
            .div_ceil(std::mem::size_of::<usize>())
            .max(1);
        let mut storage = vec![0_usize; words];
        // SAFETY: usize allocation provides QUERY_SERVICE_CONFIGW alignment, size is from SCM.
        unsafe {
            QueryServiceConfigW(
                service.0,
                Some(storage.as_mut_ptr().cast()),
                (words * std::mem::size_of::<usize>()) as u32,
                &raw mut needed,
            )
        }
        .map_err(|e| native_error("inspect_service", e))?;
        // SAFETY: successful query initialized this structure and its NUL-terminated strings inside storage.
        let config = unsafe { &*storage.as_ptr().cast::<QUERY_SERVICE_CONFIGW>() };
        let binary_path = unsafe { config.lpBinaryPathName.to_string() }
            .map_err(|e| Failure::new("inspect_service", 13, e.to_string()))?;
        let account = if config.lpServiceStartName.is_null() {
            String::new()
        } else {
            // SAFETY: pointer belongs to the successful query's live buffer.
            unsafe { config.lpServiceStartName.to_string() }
                .map_err(|e| Failure::new("inspect_service", 13, e.to_string()))?
        };
        let mut delayed = SERVICE_DELAYED_AUTO_START_INFO::default();
        // SAFETY: the initialized structure supplies aligned writable bytes for the exact API layout.
        let buffer = unsafe {
            std::slice::from_raw_parts_mut(
                (&raw mut delayed).cast::<u8>(),
                std::mem::size_of_val(&delayed),
            )
        };
        unsafe {
            QueryServiceConfig2W(
                service.0,
                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                Some(buffer),
                &raw mut needed,
            )
        }
        .map_err(|e| native_error("inspect_service", e))?;
        let mut status = SERVICE_STATUS::default();
        // SAFETY: output status is initialized and service remains open.
        unsafe { QueryServiceStatus(service.0, &raw mut status) }
            .map_err(|e| native_error("inspect_service", e))?;
        Ok(Some(Observation {
            config: Configuration {
                start_type: config.dwStartType.0,
                delayed: delayed.fDelayedAutostart.as_bool(),
                binary_path,
                account,
            },
            state: status.dwCurrentState.0,
            checkpoint: status.dwCheckPoint,
            wait_hint: Duration::from_millis(u64::from(status.dwWaitHint)),
        }))
    }
    fn set_start_type(&mut self, value: u32) -> Result<(), Failure> {
        let service = open_service(SERVICE_CHANGE_CONFIG, "configure_start_type")?;
        // SAFETY: only start type changes; all other configuration fields are NO_CHANGE/NULL.
        unsafe {
            ChangeServiceConfigW(
                service.0,
                ENUM_SERVICE_TYPE(SERVICE_NO_CHANGE),
                SERVICE_START_TYPE(value),
                SERVICE_ERROR(SERVICE_NO_CHANGE),
                PCWSTR::null(),
                PCWSTR::null(),
                None,
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
                PCWSTR::null(),
            )
        }
        .map_err(|e| native_error("configure_start_type", e))
    }
    fn set_delayed(&mut self, value: bool) -> Result<(), Failure> {
        let service = open_service(SERVICE_CHANGE_CONFIG, "restore_delayed_auto_start")?;
        let delayed = SERVICE_DELAYED_AUTO_START_INFO {
            fDelayedAutostart: value.into(),
        };
        // SAFETY: pointer has the exact structure type and remains alive for the synchronous call.
        unsafe {
            ChangeServiceConfig2W(
                service.0,
                SERVICE_CONFIG_DELAYED_AUTO_START_INFO,
                Some((&raw const delayed).cast()),
            )
        }
        .map_err(|e| native_error("restore_delayed_auto_start", e))
    }
    fn stop(&mut self) -> Result<(), Failure> {
        let service = open_service(SERVICE_STOP, "stop_service")?;
        let mut status = SERVICE_STATUS::default();
        // SAFETY: fixed service handle, normal STOP control and valid output buffer.
        match unsafe { ControlService(service.0, SERVICE_CONTROL_STOP, &raw mut status) } {
            Ok(()) => Ok(()),
            Err(error) => {
                let error = native_error("stop_service", error);
                if error.code == 1062 {
                    Ok(())
                } else {
                    Err(error)
                }
            }
        }
    }
    fn processes(&mut self) -> Result<Vec<Process>, Failure> {
        // SAFETY: process snapshot is owned by this call and closed by ProcessHandle.
        let snapshot = ProcessHandle(
            unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }.map_err(|e| {
                process_error(
                    "inspect_processes",
                    "CreateToolhelp32Snapshot",
                    "phase=enumeration",
                    e,
                )
            })?,
        );
        let mut entry = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        // SAFETY: PROCESSENTRY32W has dwSize set; iterator writes only that structure.
        if let Err(error) = unsafe { Process32FirstW(snapshot.0, &raw mut entry) } {
            if native_error("inspect_processes", error.clone()).code == 18 {
                return Ok(vec![]);
            }
            return Err(process_error(
                "inspect_processes",
                "Process32FirstW",
                "phase=enumeration",
                error,
            ));
        }
        let mut processes = Vec::new();
        loop {
            let length = entry
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(entry.szExeFile.len());
            let name = String::from_utf16_lossy(&entry.szExeFile[..length]);
            if PROCESS_NAMES
                .iter()
                .any(|known| name.eq_ignore_ascii_case(known))
            {
                // SAFETY: 只请求查询与同步权限，成功打开的句柄在全部路径关闭。
                match unsafe {
                    OpenProcess(
                        PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                        false,
                        entry.th32ProcessID,
                    )
                } {
                    Ok(handle) => {
                        let handle = ProcessHandle(handle);
                        if let Some(path) =
                            image_path(handle.0, entry.th32ProcessID, &name, "enumeration")?
                        {
                            processes.push(Process {
                                pid: entry.th32ProcessID,
                                path,
                            });
                        }
                    }
                    Err(error) => {
                        let context =
                            format!("pid={} name={name} phase=enumeration", entry.th32ProcessID);
                        let error =
                            process_error("inspect_processes", "OpenProcess", &context, error);
                        if error.code != 87 {
                            return Err(error);
                        }
                    }
                }
            }
            // SAFETY: same initialized iterator structure and live snapshot.
            if let Err(error) = unsafe { Process32NextW(snapshot.0, &raw mut entry) } {
                let error = native_error("inspect_processes", error);
                if error.code == 18 {
                    break;
                }
                let mut error = error;
                error.detail = format!(
                    "Process32NextW phase=enumeration; Win32 {}: {}",
                    error.code, error.detail
                );
                tracing::warn!(code = error.code, detail = %error.detail, "认证进程快照迭代失败");
                return Err(error);
            }
        }
        Ok(processes)
    }
    fn terminate(&mut self, process: &Process, deadline: Instant) -> Result<(), Failure> {
        remaining(deadline, "cleanup_processes")?;
        let name = process
            .path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("aTrust");
        let context = format!("pid={} name={name} phase=terminate", process.pid);
        // SAFETY: open only the selected process; full image is read again from this handle before termination.
        let handle = match unsafe {
            OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                false,
                process.pid,
            )
        } {
            Ok(handle) => ProcessHandle(handle),
            Err(error) => {
                let error = process_error("cleanup_processes", "OpenProcess", &context, error);
                return if error.code == 87 { Ok(()) } else { Err(error) };
            }
        };
        let Some(path) = image_path(handle.0, process.pid, name, "terminate_recheck")? else {
            return Ok(());
        };
        if normalized_path(&path) != normalized_path(&process.path) {
            return Ok(());
        }
        remaining(deadline, "cleanup_processes")?;
        // SAFETY: handle belongs to the exact installation process selected by orchestration.
        if let Err(error) = unsafe { TerminateProcess(handle.0, 0) } {
            let error = process_error("cleanup_processes", "TerminateProcess", &context, error);
            return accept_exited_process(error, || {
                process_exited(handle.0, "cleanup_processes", &context)
            });
        }
        let wait_ms = remaining(deadline, "cleanup_processes")?
            .as_millis()
            .min(u128::from(u32::MAX - 1)) as u32;
        // SAFETY: SYNCHRONIZE was requested and the process handle remains live.
        let result = unsafe { WaitForSingleObject(handle.0, wait_ms) };
        if result == WAIT_OBJECT_0 {
            Ok(())
        } else if result == WAIT_FAILED {
            Err(process_error(
                "cleanup_processes",
                "WaitForSingleObject",
                &context,
                windows::core::Error::from_thread(),
            ))
        } else {
            Err(Failure::new(
                "cleanup_processes",
                1460,
                "认证残余进程退出超时",
            ))
        }
    }
}
