//! 本次停止动作与仍由真实资源 owner 持有的义务；仅为内存回执。
use crate::connect_ledger::{EntryState, LedgerDomain, RollbackOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopActionState {
    Completed,
    Retained,
    NotAttempted,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopAction {
    pub domain: LedgerDomain,
    pub step: &'static str,
    pub state: StopActionState,
    pub error: Option<String>,
}

/// 只引用持有者身份，资源及重试权限继续留在原 owner 中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StopObligation {
    pub domain: LedgerDomain,
    pub owner: String,
    pub step: &'static str,
    pub blocks_new_connect: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StopReport {
    pub stopped: bool,
    pub actions: Vec<StopAction>,
    pub remaining: Vec<StopObligation>,
}

impl StopReport {
    pub fn succeeded(&self) -> bool {
        self.stopped
            && self.remaining.is_empty()
            && !self.actions.iter().any(|a| {
                matches!(
                    a.state,
                    StopActionState::Failed | StopActionState::NotAttempted
                )
            })
    }
    pub fn blocks_new_connect(&self) -> bool {
        self.remaining.iter().any(|o| o.blocks_new_connect)
    }
    pub(crate) fn action(
        &mut self,
        domain: LedgerDomain,
        step: &'static str,
        result: &Result<(), String>,
    ) {
        self.actions.push(StopAction {
            domain,
            step,
            state: if result.is_ok() {
                StopActionState::Completed
            } else {
                StopActionState::Failed
            },
            error: result.as_ref().err().cloned(),
        });
    }
    pub(crate) fn from_rollback(outcome: &RollbackOutcome) -> Self {
        let actions = outcome
            .entries
            .iter()
            .map(|entry| StopAction {
                domain: entry.domain,
                step: match entry.state {
                    EntryState::Unresolved(step) => step,
                    _ => entry.domain.id(),
                },
                state: match entry.state {
                    EntryState::Retained => StopActionState::Retained,
                    EntryState::Unresolved(_) => StopActionState::Failed,
                    _ => StopActionState::Completed,
                },
                error: entry.detail.clone(),
            })
            .collect();
        Self {
            stopped: true,
            actions,
            remaining: Vec::new(),
        }
    }
    pub(crate) fn merge(&mut self, mut other: Self) {
        self.actions.append(&mut other.actions);
        self.remaining.append(&mut other.remaining);
    }
    pub fn result(&self) -> Result<(), String> {
        if self.succeeded() {
            return Ok(());
        }
        let mut errors: Vec<_> = self
            .actions
            .iter()
            .filter_map(|a| a.error.clone())
            .collect();
        if !self.stopped {
            errors.push("stop_not_completed".into());
        }
        for obligation in &self.remaining {
            errors.push(format!(
                "{}:{}:pending",
                obligation.domain.id(),
                obligation.step
            ));
        }
        Err(errors.join("; "))
    }
}
