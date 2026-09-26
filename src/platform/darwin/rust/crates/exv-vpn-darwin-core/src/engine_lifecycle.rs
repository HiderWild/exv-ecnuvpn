//! E2 的 `Ready → Observe` 测试切片。
//!
//! 本模块仅在测试构建中存在。它通过已有 IPC bootstrap record/state machine 约束 Core 在
//! 合法 `Ready` 后才形成 root Engine peer 并进入 Observe；真实 Authorization、pipe、UDS
//! 与 Engine 启动 adapter 仍由后续已批准的执行任务实现。

#![cfg(test)]

use std::collections::VecDeque;

use exv_vpn_darwin_ipc::{
    bootstrap::{EngineBootstrapExit, EngineBootstrapRecord, EngineBootstrapSequence},
    peer::ExpectedPeer,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EngineLifecycleError {
    BootstrapProtocol,
    BootstrapRejected,
    Observe,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FakeReadOutcome {
    Record(EngineBootstrapRecord),
    Eof,
    Truncated,
    Timeout,
    Io,
    InvalidVersion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FakeReadError {
    Eof,
    Truncated,
    Timeout,
    Io,
    InvalidVersion,
}

struct FakeReader {
    outcomes: VecDeque<FakeReadOutcome>,
}

impl FakeReader {
    fn new(outcomes: impl IntoIterator<Item = FakeReadOutcome>) -> Self {
        Self {
            outcomes: outcomes.into_iter().collect(),
        }
    }

    fn read_record(&mut self) -> Result<EngineBootstrapRecord, FakeReadError> {
        match self.outcomes.pop_front().unwrap_or(FakeReadOutcome::Eof) {
            FakeReadOutcome::Record(record) => Ok(record),
            FakeReadOutcome::Eof => Err(FakeReadError::Eof),
            FakeReadOutcome::Truncated => Err(FakeReadError::Truncated),
            FakeReadOutcome::Timeout => Err(FakeReadError::Timeout),
            FakeReadOutcome::Io => Err(FakeReadError::Io),
            FakeReadOutcome::InvalidVersion => Err(FakeReadError::InvalidVersion),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FakeObserveFailure;

trait Observer {
    fn observe(&mut self, expected_engine: ExpectedPeer) -> Result<(), FakeObserveFailure>;
}

fn observe_after_ready(
    reader: &mut FakeReader,
    observer: &mut impl Observer,
) -> Result<(), EngineLifecycleError> {
    let mut sequence = EngineBootstrapSequence::new();
    let first = reader
        .read_record()
        .map_err(|_| EngineLifecycleError::BootstrapProtocol)?;
    let _ready_transition = sequence
        .accept(first)
        .map_err(|_| EngineLifecycleError::BootstrapProtocol)?;
    let EngineBootstrapRecord::Ready { pid } = first else {
        return Err(EngineLifecycleError::BootstrapProtocol);
    };

    observer
        .observe(ExpectedPeer::new(0, pid))
        .map_err(|_| EngineLifecycleError::Observe)?;

    loop {
        match reader.read_record() {
            Ok(record) => {
                let _terminal_transition = sequence
                    .accept(record)
                    .map_err(|_| EngineLifecycleError::BootstrapProtocol)?;
            }
            Err(FakeReadError::Eof) => {
                let exit = sequence
                    .finish_on_eof()
                    .map_err(|_| EngineLifecycleError::BootstrapProtocol)?;
                return match exit {
                    EngineBootstrapExit::Ok { .. } => Ok(()),
                    EngineBootstrapExit::Error { .. } => {
                        Err(EngineLifecycleError::BootstrapRejected)
                    }
                };
            }
            Err(
                FakeReadError::Truncated
                | FakeReadError::Timeout
                | FakeReadError::Io
                | FakeReadError::InvalidVersion,
            ) => return Err(EngineLifecycleError::BootstrapProtocol),
        }
    }
}
