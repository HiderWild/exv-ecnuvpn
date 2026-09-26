#![cfg(test)]

//! E3 Engine bootstrap 的纯 fake 行为契约。
//!
//! 此模块只在测试构建中存在。它不读取 runtime/ticket 文件、不检查真实 uid/euid、不创建
//! socket、不写 communications pipe，也不启动 server。它只用现有的
//! [`EngineBootstrapRecord`] 记录已批准的 `Ready → terminal` 行为，以便在 E4 固定
//! canonical Engine path 前锁定 fail-closed 顺序。

use exv_vpn_darwin_ipc::bootstrap::EngineBootstrapRecord;
use exv_vpn_wire::generated as wire;

const TEST_RUNTIME_DIR: &str = "/private/tmp/e3";
const OWNER_UID: u32 = 501;
const CORE_PID: u32 = 4_321;
const ENGINE_PID: u32 = 5_432;
const MAX_VALID_NUMERIC: u32 = 2_147_483_647;
const EXIT_ERROR_CODE: u32 = 7;

/// Engine bootstrap 的 test-only 内部断言分类。
///
/// 它从不编码到 bootstrap pipe，也不跨进程或跨 crate 暴露给 Core；真实 I/O 接线不在本
/// 测试模块内。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EngineBootstrapError {
    ArgsInvalid,
    EuidRequired,
    OriginUidMismatch,
    RuntimeDirInvalid,
    TicketOpen,
    TicketMetadata,
    TicketMalformed,
    TicketCleanupRefused,
    SocketPublish,
    ReadyPipe,
    SocketCleanup,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BootstrapArgs<'a> {
    runtime_dir: &'a str,
    owner_uid: u32,
    core_pid: u32,
}

fn parse_args<'a>(args: &'a [&'a str]) -> Result<BootstrapArgs<'a>, EngineBootstrapError> {
    if args.len() != 6
        || args[0] != "--runtime-dir"
        || args[1] != TEST_RUNTIME_DIR
        || args[2] != "--owner-uid"
        || args[4] != "--core-pid"
    {
        return Err(EngineBootstrapError::ArgsInvalid);
    }

    let owner_uid = parse_positive_i32(args[3]).ok_or(EngineBootstrapError::ArgsInvalid)?;
    let core_pid = parse_positive_i32(args[5]).ok_or(EngineBootstrapError::ArgsInvalid)?;
    Ok(BootstrapArgs {
        runtime_dir: args[1],
        owner_uid,
        core_pid,
    })
}

fn parse_positive_i32(value: &str) -> Option<u32> {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let parsed = value.parse::<u32>().ok()?;
    (1..=MAX_VALID_NUMERIC).contains(&parsed).then_some(parsed)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FakeIdentity {
    effective_uid: u32,
    real_uid: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FakeTicket {
    owner_uid: u32,
    core_pid: u32,
}

/// 每次 fake run 只允许一个故障点，避免测试自行定义双故障优先级。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FakeFault {
    None,
    RuntimeDir,
    TicketOpen,
    TicketMetadata,
    TicketMalformed,
    TicketCleanupRefused,
    RuntimeSeal,
    SocketPublish,
    ReadyPipeClosed,
    Observe,
    SocketCleanup,
    TerminalPipeClosed,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FakeEvent {
    RuntimeOpened,
    TicketConsumed,
    RuntimeSealed,
    SocketPublished,
    PipeWrote(EngineBootstrapRecord),
    PipeWriteRefused(EngineBootstrapRecord),
    ObserveLookupKeyNone,
    SocketCleaned,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FakeObserveFailure;

/// 纯内存依赖；事件只表达 bootstrap 过程，故意没有 operation/lease/network/resource mutation。
struct FakeDependencies {
    identity: FakeIdentity,
    ticket: FakeTicket,
    fault: FakeFault,
    engine_pid: u32,
    events: Vec<FakeEvent>,
}

impl Default for FakeDependencies {
    fn default() -> Self {
        Self {
            identity: FakeIdentity {
                effective_uid: 0,
                real_uid: OWNER_UID,
            },
            ticket: FakeTicket {
                owner_uid: OWNER_UID,
                core_pid: CORE_PID,
            },
            fault: FakeFault::None,
            engine_pid: ENGINE_PID,
            events: Vec::new(),
        }
    }
}

impl FakeDependencies {
    fn with_fault(fault: FakeFault) -> Self {
        Self {
            fault,
            ..Self::default()
        }
    }

    fn open_runtime(&mut self, runtime_dir: &str) -> Result<(), EngineBootstrapError> {
        assert_eq!(
            runtime_dir, TEST_RUNTIME_DIR,
            "fake accepts one opaque test path"
        );
        if self.fault == FakeFault::RuntimeDir {
            return Err(EngineBootstrapError::RuntimeDirInvalid);
        }
        self.events.push(FakeEvent::RuntimeOpened);
        Ok(())
    }

    fn consume_ticket(&mut self) -> Result<FakeTicket, EngineBootstrapError> {
        let error = match self.fault {
            FakeFault::TicketOpen => Some(EngineBootstrapError::TicketOpen),
            FakeFault::TicketMetadata => Some(EngineBootstrapError::TicketMetadata),
            FakeFault::TicketMalformed => Some(EngineBootstrapError::TicketMalformed),
            FakeFault::TicketCleanupRefused => Some(EngineBootstrapError::TicketCleanupRefused),
            _ => None,
        };
        if let Some(error) = error {
            return Err(error);
        }
        self.events.push(FakeEvent::TicketConsumed);
        Ok(self.ticket)
    }

    fn seal_runtime(&mut self) -> Result<(), EngineBootstrapError> {
        if self.fault == FakeFault::RuntimeSeal {
            return Err(EngineBootstrapError::RuntimeDirInvalid);
        }
        self.events.push(FakeEvent::RuntimeSealed);
        Ok(())
    }

    fn publish_socket(&mut self) -> Result<(), EngineBootstrapError> {
        if self.fault == FakeFault::SocketPublish {
            return Err(EngineBootstrapError::SocketPublish);
        }
        self.events.push(FakeEvent::SocketPublished);
        Ok(())
    }

    fn write_ready(&mut self, record: EngineBootstrapRecord) -> Result<(), EngineBootstrapError> {
        if self.fault == FakeFault::ReadyPipeClosed {
            self.events.push(FakeEvent::PipeWriteRefused(record));
            return Err(EngineBootstrapError::ReadyPipe);
        }
        self.events.push(FakeEvent::PipeWrote(record));
        Ok(())
    }

    fn observe(
        &mut self,
        request: &wire::ObserveOwnedStateRequest,
    ) -> Result<(), FakeObserveFailure> {
        if request.lookup_key.is_some() {
            return Err(FakeObserveFailure);
        }
        self.events.push(FakeEvent::ObserveLookupKeyNone);
        if self.fault == FakeFault::Observe {
            return Err(FakeObserveFailure);
        }
        Ok(())
    }

    fn cleanup_socket(&mut self) -> Result<(), EngineBootstrapError> {
        if self.fault == FakeFault::SocketCleanup {
            return Err(EngineBootstrapError::SocketCleanup);
        }
        self.events.push(FakeEvent::SocketCleaned);
        Ok(())
    }

    fn write_terminal(&mut self, record: EngineBootstrapRecord) {
        if self.fault == FakeFault::TerminalPipeClosed {
            self.events.push(FakeEvent::PipeWriteRefused(record));
        } else {
            self.events.push(FakeEvent::PipeWrote(record));
        }
    }

    fn successful_records(&self) -> Vec<EngineBootstrapRecord> {
        self.events
            .iter()
            .filter_map(|event| match event {
                FakeEvent::PipeWrote(record) => Some(*record),
                _ => None,
            })
            .collect()
    }
}

/// 执行一次没有真实系统副作用的 E3 bootstrap 顺序。
fn run_fake_lifecycle(
    argv: &[&str],
    dependencies: &mut FakeDependencies,
) -> Result<(), EngineBootstrapError> {
    let bootstrap_args = parse_args(argv)?;
    if dependencies.identity.effective_uid != 0 {
        return Err(EngineBootstrapError::EuidRequired);
    }
    if dependencies.identity.real_uid != bootstrap_args.owner_uid {
        return Err(EngineBootstrapError::OriginUidMismatch);
    }

    dependencies.open_runtime(bootstrap_args.runtime_dir)?;
    let ticket = dependencies.consume_ticket()?;
    if ticket.owner_uid != bootstrap_args.owner_uid || ticket.core_pid != bootstrap_args.core_pid {
        return Err(EngineBootstrapError::TicketMalformed);
    }
    dependencies.seal_runtime()?;
    dependencies.publish_socket()?;

    let ready = EngineBootstrapRecord::Ready {
        pid: dependencies.engine_pid,
    };
    if let Err(error) = dependencies.write_ready(ready) {
        let _ = dependencies.cleanup_socket();
        return Err(error);
    }

    let observe_failed = dependencies
        .observe(&wire::ObserveOwnedStateRequest { lookup_key: None })
        .is_err();
    let cleanup_failed = dependencies.cleanup_socket().is_err();
    let terminal = if observe_failed || cleanup_failed {
        EngineBootstrapRecord::ExitError {
            pid: dependencies.engine_pid,
            code: EXIT_ERROR_CODE,
        }
    } else {
        EngineBootstrapRecord::ExitOk {
            pid: dependencies.engine_pid,
        }
    };
    dependencies.write_terminal(terminal);

    if cleanup_failed {
        Err(EngineBootstrapError::SocketCleanup)
    } else {
        Ok(())
    }
}
