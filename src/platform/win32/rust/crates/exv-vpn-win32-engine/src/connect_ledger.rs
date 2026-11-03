//! 连接尝试的进程内事实记录；资源由 runtime 的同一 owner 组持有。

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceAdmission {
    Unwired,
    Skipped { connection_id: Option<String> },
    Created { connection_id: String },
    Replaced { connection_id: String },
    Existing { connection_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerDomain {
    SessionLease,
    Nic,
    RouteFamily,
    DataPlane,
    Nrpt,
    Trace,
}

impl LedgerDomain {
    /// trace 和诊断只输出稳定域名，不携 operation、错误详情或资源原始数据。
    pub const fn id(self) -> &'static str {
        match self {
            Self::SessionLease => "session_lease",
            Self::Nic => "nic",
            Self::RouteFamily => "route_family",
            Self::DataPlane => "data_plane",
            Self::Nrpt => "nrpt",
            Self::Trace => "trace",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryState {
    Registered,
    Committed,
    Compensated,
    Retained,
    Unresolved(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttemptEnd {
    Committed,
    Failed,
    Cancelled,
    Panicked,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerEntry {
    pub sequence: usize,
    pub domain: LedgerDomain,
    pub persistent: bool,
    pub state: EntryState,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectLedger {
    pub attempt_id: Vec<u8>,
    pub trace: TraceAdmission,
    pub entries: Vec<LedgerEntry>,
    pub ended: Option<AttemptEnd>,
}

#[derive(Debug, Default)]
pub struct AttemptLedger {
    pub active: Option<ConnectLedger>,
    pub last_closed: Option<ConnectLedger>,
    pub last_rollback: Option<RollbackOutcome>,
}

/// 每个结果都来自已经调用的原语；Trace 在阶段 5 收口前不制造清理结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompensationResult {
    pub sequence: Option<usize>,
    pub domain: LedgerDomain,
    pub state: EntryState,
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct RollbackOutcome {
    pub attempt_id: Vec<u8>,
    pub entries: Vec<CompensationResult>,
    pub unresolved: Vec<(LedgerDomain, String)>,
}

impl RollbackOutcome {
    pub fn unresolved_domains(&self) -> Vec<String> {
        let mut domains = Vec::new();
        for (domain, _) in &self.unresolved {
            let id = domain.id().to_owned();
            if !domains.contains(&id) {
                domains.push(id);
            }
        }
        domains
    }

    pub fn resolved(&self) -> bool {
        self.unresolved.is_empty()
    }

    pub fn observe(
        &mut self,
        sequence: Option<usize>,
        domain: LedgerDomain,
        result: Result<(), String>,
        retained: bool,
        category: &'static str,
    ) {
        let (state, detail) = match result {
            Ok(()) => (
                if retained {
                    EntryState::Retained
                } else {
                    EntryState::Compensated
                },
                None,
            ),
            Err(detail) => {
                self.unresolved.push((domain, detail.clone()));
                (EntryState::Unresolved(category), Some(detail))
            }
        };
        self.entries.push(CompensationResult {
            sequence,
            domain,
            state,
            detail,
        });
    }
}

impl AttemptLedger {
    /// 已有活动记录时不覆盖；调用方仍按既有资源裁决处理请求。
    pub fn begin(&mut self, id: &[u8], trace: TraceAdmission) -> bool {
        if self.active.is_some() {
            return false;
        }
        let has_trace = matches!(
            &trace,
            TraceAdmission::Created { .. }
                | TraceAdmission::Replaced { .. }
                | TraceAdmission::Existing { .. }
                | TraceAdmission::Skipped {
                    connection_id: Some(_)
                }
        );
        self.active = Some(ConnectLedger {
            attempt_id: id.to_vec(),
            trace,
            entries: Vec::new(),
            ended: None,
        });
        self.last_rollback = None;
        if has_trace {
            self.register(id, LedgerDomain::Trace, false);
        }
        true
    }

    /// 序号在本次实际登记中分配，供后续逐项结果定位；不包含资源句柄。
    pub fn register(&mut self, id: &[u8], domain: LedgerDomain, persistent: bool) -> Option<usize> {
        let ledger = self
            .active
            .as_mut()
            .filter(|ledger| ledger.attempt_id == id)?;
        let sequence = ledger.entries.len();
        ledger.entries.push(LedgerEntry {
            sequence,
            domain,
            persistent,
            state: EntryState::Registered,
        });
        Some(sequence)
    }

    /// 只接收已经观察到的局部结果，未尝试的条目保持 Registered。
    pub fn record_result(&mut self, id: &[u8], sequence: usize, state: EntryState) -> bool {
        let Some(entry) = self
            .active
            .as_mut()
            .filter(|ledger| ledger.attempt_id == id)
            .and_then(|ledger| ledger.entries.get_mut(sequence))
        else {
            return false;
        };
        entry.state = state;
        true
    }

    /// 收口记录不代表资源清理完成。只保留最近一次记录，避免历史囤积。
    pub fn finish(&mut self, id: &[u8], end: AttemptEnd) -> bool {
        if !self
            .active
            .as_ref()
            .is_some_and(|ledger| ledger.attempt_id == id)
        {
            return false;
        }
        let mut ledger = self.active.take().expect("matched active record");
        if end == AttemptEnd::Committed {
            for entry in &mut ledger.entries {
                if entry.state == EntryState::Registered {
                    entry.state = EntryState::Committed;
                }
            }
        }
        ledger.ended = Some(end);
        self.last_closed = Some(ledger);
        true
    }
}

impl ConnectLedger {
    pub fn reverse_entries(&self) -> impl Iterator<Item = &LedgerEntry> {
        self.entries.iter().rev()
    }
}
