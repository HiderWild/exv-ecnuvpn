
//! 连接门（T4 连接时序接线，计划 §2.1–§2.4 / §三 / 2026-09-27 计划；
//! T7 升级迁移扩展）。
//!
//! 职责：把 T1 的 [`ConnectionTraceStore`] 与 T2/T3 的预检接进**连接受理点**
//! （`grpc_server::apply_tunnel`，先于 admission 与任何系统变更）：
//!
//! 1. **受理即建痕**：连接请求被接受时创建 `trace`（connecting 态）——先于登录、
//!    网卡、路由等一切系统变更；core 不落盘（D1-b：engine 单写者）。
//! 2. **痕在即预检**：`create` 撞上遗留（含损坏）→ 运行一次预检；未决发现阻断
//!    本次连接（[`GateRejection::reason_ids`] 稳定 id，UI 文案后置）；预检干净
//!    → 删除遗留 → 重建新痕 → 放行。
//! 3. **装配回写**：[`RealTunnelRuntime`](crate::tunnel_runtime::RealTunnelRuntime)
//!    在装配成功（`install_live` 之后）经 [`ConnectGate::mark_live`] 推 Live 并回写
//!    网卡/NRPT owner/engine PID；T7 起同一锚点回写**版本标记**（§三：版本升级
//!    首连强制预检的"上次连接版本"事实源，[`crate::version_marker`]）。
//!
//! **T7 触发源合并（§三）**：痕在触发（T4）之外新增两类，任一命中即预检、同一
//! 次受理内只跑一轮：
//! - **版本升级**：当前 engine 版本 != 版本标记（标记缺失 = 旧版升级/全新安装、
//!   损坏 = 版本未知，均保守触发）→ 即使无痕也强制预检；预检通过且连接建立
//!   （`mark_live`）后更新标记——装配失败不更新，下次连接重复触发（保守）。
//! - **engine 启动后首连**：本 engine 进程生命周期内尚未跑过预检的首连预检一次
//!   （进程内原子标志）。服务形态 = 服务启动后首连；oneshot 每次 provision 即新
//!   进程 = 天然覆盖 provision 触发（§三第 4 条；respawn 0 网卡/0 路由硬断言的
//!   吸收即落在此——预检七域取代退出期硬断言，退出期兜底仍由 main.rs 既有
//!   清理路径承担）。
//!
//! 组装失败/回滚**不删痕**（connecting 态 = 善后未竟，留给下次预检裁定，§2.4）。
//! 删除权的其余情形（teardown 策略 / 无头 R1 终态 / core 崩溃环 / 断开路径验证
//! 后删）归 T5；本模块只在「预检干净后重建」与「本 engine 自留状态下的重建」
//! 两个位置行使删除（前者 = 预检复验代替善后验证；后者见
//! [`ConnectGate::replace_connect_trace`]）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::connection_trace::{
    ConnectionTraceStore, TraceEngineMode, TraceKind, TraceOnCoreExit, TracePatch, TraceReadOutcome,
    TraceRecord, TraceReconnectPolicy, TraceState, TraceStoreError,
};
use crate::log_sink::{LogLevel, LogSink};
use crate::preflight::{self, PreflightInputs};

/// 连接门预检执行器（T7）：与 T5 善后验证器（[`crate::aftermath::AftermathVerifier`]
/// ）同一接缝形态——生产 = [`preflight::run_preflight_for`] 包装（user_sid/命名空间
/// 取门自身事实源），测试注入假报告（预检真实行为在 preflight.rs 单测与 T13 验收
/// 门禁覆盖，单测不触真实系统）。
type PreflightRunner = crate::aftermath::AftermathVerifier;

/// 连接门放行结局。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAdmission {
    /// 干净受理：无遗留痕且无 T7 触发源命中，本次 create 直接落盘。
    CreatedFresh,
    /// 无遗留痕，但 T7 触发源（版本升级 / engine 首连）命中 → 预检干净后放行
    /// （§三；痕为新落，非删旧建新）。
    FreshAfterPreflight,
    /// 遗留痕经预检（干净 → 删旧建新）后放行。
    RecreatedAfterPreflight,
    /// 本 engine 自留状态（Live 接力 / Paused 保网卡，D12）下的删旧建新——
    /// 预检前提「无存活 engine 会话」不成立，不跑预检。
    RecreatedInPlace,
}

/// 连接门拒绝（结构化稳定 id；UI 文案按 id 路由，后置阶段）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateRejection {
    /// 阻断原因稳定 id 清单（见 [`preflight::blocking_reasons`] 与本模块的
    /// 载体类 id：`trace_store_unavailable` / `trace_user_sid_missing` /
    /// `trace_create_failed`）。
    pub reason_ids: Vec<&'static str>,
    /// 人读摘要（落日志；不上 wire）。
    pub summary: String,
}

impl GateRejection {
    /// 载体不可用类拒绝。
    fn carrier(reason_id: &'static str, summary: String) -> Self {
        Self {
            reason_ids: vec![reason_id],
            summary,
        }
    }
}

/// 连接受理信封（engine 侧从 wire 信封 + 传输层已验证事实组装）。
#[derive(Debug, Clone)]
pub struct ConnectEnvelope {
    /// 受理本次连接的 core PID（**传输层已验证 peer 的 PID**，非自报字段）。
    pub core_pid: u32,
    /// core 背后的 UI PID（wire `ui_pid`；0 = 旧 core 未上报）。
    pub ui_pid: u32,
    /// core 退出策略（wire `on_core_exit`；UNSPECIFIED 已在上游折算为 Teardown）。
    pub on_core_exit: TraceOnCoreExit,
    /// 重连策略镜像（wire `reconnect_policy`；缺省已在上游折算）。
    pub reconnect: TraceReconnectPolicy,
    /// 隧道种类（凭据类型决定）。
    pub kind: TraceKind,
}

/// 连接门：持有 trace store + engine 形态 + 发起用户 SID + 用户配置目录。
///
/// 由 `grpc_server::with_real_tunnel` 构建并注入服务与运行时两侧；测试构造
/// （`HelperControlService::new` 等）不注入（gate = None，连接路径维持既有行为）。
pub struct ConnectGate {
    /// trace store（`%ProgramData%\ExvVpn\connection`；打开失败 = None，本会话
    /// 无痕保护——受理时按载体不可用拒绝，见 [`GateRejection`]）。
    store: Option<ConnectionTraceStore>,
    /// engine 形态（service / oneshot）。
    engine_mode: TraceEngineMode,
    /// 发起用户 SID（trace 文件名段；服务形态必传）。
    user_sid: Option<String>,
    /// 用户配置目录（预检校园命名空间与痕迹 `server` 字段的读取源）。
    config_dir: PathBuf,
    /// T7 §三：本 engine 进程生命周期内预检是否已执行（"engine 启动后首连"
    /// 触发源；oneshot 每次 provision 即新进程，天然覆盖 provision 触发）。
    preflight_done: AtomicBool,
    /// 预检执行器（生产 = 真实 `run_preflight_for` 包装；测试注入假报告——
    /// 与 T5 善后验证器同一接缝形态，见 [`PreflightRunner`]）。
    preflight_runner: PreflightRunner,
}

impl ConnectGate {
    /// 构建连接门。store 打开失败不 panic（engine 仍可服务既有连接的拆除/状态），
    /// 只记一次告警；此后每次受理将按 `trace_store_unavailable` 拒绝新连接
    /// （fail closed：无法落痕的连接不接受——善后保障优先）。
    #[must_use]
    pub fn open(
        engine_mode: TraceEngineMode,
        user_sid: Option<String>,
        config_dir: PathBuf,
        log: &LogSink,
    ) -> Self {
        let store = ConnectionTraceStore::machine_default(user_sid.as_deref()).map_err(|error| {
            log.emit(
                LogLevel::Warn,
                "engine",
                "connect.gate.store_unavailable",
                "连接痕迹目录不可用；新连接将被拒绝（fail closed）",
                &[("detail", format!("{error:?}").as_str())],
            );
            error
        });
        let runner = Self::real_preflight_runner(&user_sid, &config_dir);
        Self::with_store(store.ok(), engine_mode, user_sid, config_dir, runner)
    }

    /// 指定 store 载体上的门（crate 内组装接缝：生产经 [`Self::open`] 打开机器级
    /// store，测试与自定义布局直接传入 temp 目录 store 与预检执行器；其余参数
    /// 语义同 [`Self::open`]）。
    #[must_use]
    pub(crate) fn with_store(
        store: Option<ConnectionTraceStore>,
        engine_mode: TraceEngineMode,
        user_sid: Option<String>,
        config_dir: PathBuf,
        preflight_runner: PreflightRunner,
    ) -> Self {
        Self {
            store,
            engine_mode,
            user_sid,
            config_dir,
            preflight_done: AtomicBool::new(false),
            preflight_runner,
        }
    }

    /// 生产预检执行器：包装 [`preflight::run_preflight_for`]——user_sid 取门自身
    /// 字段；校园命名空间按 config_dir **每次调用现读**（与 T4 受理路径同一事实
    /// 源，配置变更即时生效）。
    fn real_preflight_runner(
        user_sid: &Option<String>,
        config_dir: &Path,
    ) -> PreflightRunner {
        let user_sid = user_sid.clone();
        let config_dir = config_dir.to_path_buf();
        Arc::new(move |hint| {
            let user_sid = user_sid.clone();
            let config_dir = config_dir.clone();
            Box::pin(async move {
                let namespaces = campus_namespaces_in(&config_dir);
                preflight::run_preflight_for(&PreflightInputs {
                    record_hint: hint.as_ref(),
                    user_sid: user_sid.as_deref(),
                    campus_namespaces: &namespaces,
                })
                .await
            })
        })
    }

    /// 全量受理路径：create → 撞遗留（含损坏）→ 预检 → 阻断或删旧建新；
    /// T7 触发源（版本升级 / engine 首连）在**无遗留痕**时于 create 前预检
    /// （受理未放行前不落痕），有遗留痕时并入 resolve_stale 的同一轮预检。
    ///
    /// # Errors
    ///
    /// 载体不可用 / 预检未决发现阻断 / 重建失败 → [`GateRejection`]（调用方
    /// 转结构化连接拒绝，稳定 id 见 [`GateRejection::reason_ids`]）。
    pub async fn ensure_connect_trace(
        &self,
        envelope: &ConnectEnvelope,
        log: &LogSink,
    ) -> Result<GateAdmission, GateRejection> {
        let record = self.build_record(envelope);
        let sid = self.user_sid_for_write()?;
        let store = self.store_for_write()?;
        // T7 触发源评估（痕外）：版本不匹配 / 本进程首连。命中且当前**无遗留痕**
        // 时先跑预检（阻断 → 拒绝且什么都不落盘）；有遗留痕时预检并入
        // resolve_stale（hint 更完整，同代次只跑一轮）。create 之后才撞见痕的
        // 罕见竞态（单写者模型之外的外部写者）会走 resolve_stale 再跑一轮——
        // 保守方向，可接受。
        let triggers = self.extra_preflight_triggers(sid, log);
        let mut preflighted_this_admission = false;
        if !triggers.is_empty()
            && let Ok(TraceReadOutcome::Absent) = store.read(sid)
        {
            log.emit(
                    LogLevel::Info,
                    "engine",
                    "connect.gate.preflight.started",
                    "版本升级或本进程首连触发连接前只读预检（无遗留痕迹）",
                    &[("triggers", &triggers.join(","))],
                );
                let report = (self.preflight_runner)(None).await;
                self.note_preflight_ran();
                preflighted_this_admission = true;
                let reason_ids = preflight::blocking_reasons(&report);
                if !reason_ids.is_empty() {
                    log.emit(
                        LogLevel::Warn,
                        "engine",
                        "connect.gate.preflight.blocked",
                        "连接前预检发现未决残留，本次连接被拒绝",
                        &[
                            ("reason_ids", &reason_ids.join(",")),
                            ("triggers", &triggers.join(",")),
                            (
                                "elapsed_ms",
                                &report.elapsed.as_millis().to_string(),
                            ),
                        ],
                    );
                    return Err(GateRejection {
                        reason_ids,
                        summary: format!(
                            "预检未决发现（触发源 {}）：{}",
                            triggers.join(","),
                            report
                                .domains
                                .iter()
                                .filter(|domain| !domain.findings.is_empty())
                                .map(|domain| {
                                    format!("{:?}×{}", domain.domain, domain.findings.len())
                                })
                                .collect::<Vec<_>>()
                                .join("；"),
                        ),
                    });
                }
                log.emit(
                    LogLevel::Info,
                    "engine",
                    "connect.gate.preflight.passed",
                    "预检干净，放行本次连接（无遗留痕迹，直接落新痕）",
                    &[
                        ("triggers", &triggers.join(",")),
                        ("elapsed_ms", &report.elapsed.as_millis().to_string()),
                    ],
                );
        }
        match store.create(sid, &record) {
            Ok(()) => Ok(if preflighted_this_admission {
                GateAdmission::FreshAfterPreflight
            } else {
                GateAdmission::CreatedFresh
            }),
            Err(TraceStoreError::AlreadyExists) => {
                self.resolve_stale(sid, &record, log).await
            }
            Err(error) => Err(GateRejection::carrier(
                "trace_create_failed",
                format!("trace create 失败：{error:?}"),
            )),
        }
    }

    /// T7 触发源评估（痕外，§三）：返回命中的触发源 id 清单（空 = 无）。
    ///
    /// - `version_upgrade`：版本标记 != 当前 engine 版本（标记缺失 = 旧版升级
    ///   上来/全新安装；损坏/读失败 = 版本未知——全部保守触发）。
    /// - `engine_first_connect`：本 engine 进程生命周期内尚未跑过预检。
    fn extra_preflight_triggers(&self, sid: &str, log: &LogSink) -> Vec<&'static str> {
        let mut triggers: Vec<&'static str> = Vec::new();
        let marker = self
            .store
            .as_ref()
            .and_then(|store| crate::version_marker::read(store.dir(), sid));
        if marker.as_deref() != Some(crate::version_marker::ENGINE_VERSION) {
            log.emit(
                LogLevel::Info,
                "engine",
                "connect.gate.preflight.version_marker",
                "上次连接版本与当前 engine 版本不一致（或标记缺失/损坏），首连将强制预检",
                &[
                    (
                        "engine_version",
                        crate::version_marker::ENGINE_VERSION,
                    ),
                    (
                        "marker",
                        marker.as_deref().unwrap_or("<unknown>"),
                    ),
                ],
            );
            triggers.push("version_upgrade");
        }
        if !self.preflight_done.load(Ordering::Acquire) {
            log.emit(
                LogLevel::Info,
                "engine",
                "connect.gate.preflight.engine_first_connect",
                "本 engine 进程启动后首次连接，执行连接前预检",
                &[],
            );
            triggers.push("engine_first_connect");
        }
        triggers
    }

    /// 预检已在本进程内执行（消费"engine 首连"触发源；T7 §三——同代次内不重复
    /// 跑）。版本触发源由 `mark_live` 的标记回写自然消费（连接建立前标记保持
    /// 旧值 → 重试仍触发，保守方向）。
    fn note_preflight_ran(&self) {
        self.preflight_done.store(true, Ordering::Release);
    }

    /// 本 engine 自留状态下的重建（不跑预检）：删除遗留（已验证善后的旧连接）
    /// 并为本次连接落新痕。调用前提 = 遗留痕描述的是**本 engine 自己**保留的
    /// 会话/网卡（Live 接力换凭据 / Paused 保网卡重连，D12），不是孤儿。
    ///
    /// # Errors
    ///
    /// 载体不可用 / 删除或重建失败 → [`GateRejection`]。
    pub fn replace_connect_trace(
        &self,
        envelope: &ConnectEnvelope,
    ) -> Result<GateAdmission, GateRejection> {
        let record = self.build_record(envelope);
        let sid = self.user_sid_for_write()?;
        let store = self.store_for_write()?;
        if let Err(error) = store.delete(sid) {
            return Err(GateRejection::carrier(
                "trace_create_failed",
                format!("trace replace 删除遗留失败：{error:?}"),
            ));
        }
        store.create(sid, &record).map_err(|error| {
            GateRejection::carrier(
                "trace_create_failed",
                format!("trace replace 重建失败：{error:?}"),
            )
        })?;
        Ok(GateAdmission::RecreatedInPlace)
    }

    /// 装配成功回写：connecting → live + 装配字段补丁（网卡/NRPT owner/端口/PID）。
    ///
    /// T7 锚点：本方法只在 `install_live` 之后被调用（连接成功建立的准确锚点——
    /// 数据面已承载业务流），同一时刻回写**版本标记**（`mark_live` = 首次建立；
    /// R1 重连的 `update_established_at` 是既有代次的重新建立，不重复回写）。
    ///
    /// 首建锚点（诊断质量项，计划 v6.1 P1-7）：`established_at_ms` 在此回写真实
    /// 时刻（不换 `connection_id`——受理时铸造的代次不动；此前仅重连回写、首建
    /// 留 0，现场取证无法回答"何时建立"）。
    ///
    /// 失败只告警不上抛——装配已成功，痕迹/标记写失败不得拆毁活隧道；痕的
    /// connecting 残留下次预检兜底；标记未更新 → 版本触发源下次连接重复命中
    /// （多跑一轮无害预检，保守方向）。
    pub fn mark_live(&self, patch: &TracePatch, log: &LogSink) {
        let Ok(sid) = self.user_sid_for_write() else {
            return;
        };
        let Some(store) = self.store.as_ref() else {
            return;
        };
        match crate::version_marker::write(store.dir(), sid, crate::version_marker::ENGINE_VERSION)
        {
            Ok(()) => log.emit(
                LogLevel::Debug,
                "engine",
                "connect.gate.version_marker.updated",
                "连接成功建立，已记录本次连接的 engine 版本",
                &[("version", crate::version_marker::ENGINE_VERSION)],
            ),
            Err(error) => log.emit(
                LogLevel::Warn,
                "engine",
                "connect.gate.version_marker.write_failed",
                "版本标记回写失败（下次连接将重复预检，保守方向）",
                &[("detail", &error.to_string())],
            ),
        }
        if let Err(error) = store.update_state(sid, TraceState::Live) {
            self.warn_trace(log, "update_state(live)", &error);
            return;
        }
        if let Err(error) = store.mark_established(sid, unix_now_ms()) {
            self.warn_trace(log, "mark_established", &error);
        }
        if let Err(error) = store.patch_fields(sid, patch) {
            self.warn_trace(log, "patch_fields", &error);
        }
    }

    /// T6a：重连成功回写（§2.4：**engine 在重连成功时**调用；采纳路径不得触碰）。
    ///
    /// 铸造新 `connection_id` 并换新 `established_at_ms` 时刻；返回新 id（供事件/
    /// 日志关联）。失败只告警——隧道已恢复，痕迹代次陈旧不影响业务，下次回写/
    /// 预检兜底。
    #[must_use]
    pub fn update_established_at(&self, log: &LogSink) -> Option<String> {
        let sid = self.user_sid_for_write().ok()?;
        let store = self.store.as_ref()?;
        let connection_id = uuid::Uuid::new_v4().to_string();
        match store.update_established_at(sid, &connection_id, unix_now_ms()) {
            Ok(()) => Some(connection_id),
            Err(error) => {
                self.warn_trace(log, "update_established_at", &error);
                None
            }
        }
    }

    /// 遗留痕处置：预检 → 阻断或（干净）删旧建新。
    async fn resolve_stale(
        &self,
        sid: &str,
        record: &TraceRecord,
        log: &LogSink,
    ) -> Result<GateAdmission, GateRejection> {
        let Some(store) = self.store.as_ref() else {
            return Err(GateRejection::carrier(
                "trace_store_unavailable",
                "trace store 不可用".to_owned(),
            ));
        };
        // hint：遗留可读 → 供预检认领网卡/端口；损坏 → 无 hint 探测（损坏=善后未竟，
        // 预检必须看见它，这正是触发分支）。
        let hint = match store.read(sid) {
            Ok(TraceReadOutcome::Present(previous)) => Some(previous),
            Ok(TraceReadOutcome::Corrupt) => None,
            Ok(TraceReadOutcome::Absent) => None, // 罕见竞态（create 与 read 间被清）→ 按无遗留探测
            Err(error) => {
                return Err(GateRejection::carrier(
                    "trace_create_failed",
                    format!("trace 遗留读取失败：{error:?}"),
                ));
            }
        };
        // 校园命名空间来自用户配置（预检外来 NRPT 冲突匹配集；读取失败按空集探测）。
        let namespaces = self.campus_namespaces();
        log.emit(
            LogLevel::Info,
            "engine",
            "connect.gate.preflight.started",
            "检测到上次连接遗留痕迹，连接前执行只读预检",
            &[
                ("hint_present", &hint.is_some().to_string()),
                ("campus_namespaces", &namespaces.len().to_string()),
                ("triggers", "stale_trace"),
            ],
        );
        let report = (self.preflight_runner)(hint.clone()).await;
        // T7：痕在触发的这轮预检同时满足"engine 首连"触发源（同一轮，不重复跑）。
        self.note_preflight_ran();
        let reason_ids = preflight::blocking_reasons(&report);
        if !reason_ids.is_empty() {
            log.emit(
                LogLevel::Warn,
                "engine",
                "connect.gate.preflight.blocked",
                "连接前预检发现未决残留，本次连接被拒绝",
                &[
                    ("reason_ids", &reason_ids.join(",")),
                    (
                        "elapsed_ms",
                        &report.elapsed.as_millis().to_string(),
                    ),
                ],
            );
            return Err(GateRejection {
                reason_ids,
                summary: format!(
                    "预检未决发现（{}）：{}",
                    report
                        .domains
                        .iter()
                        .filter(|domain| !domain.findings.is_empty())
                        .map(|domain| format!("{:?}×{}", domain.domain, domain.findings.len()))
                        .collect::<Vec<_>>()
                        .join("；"),
                    report
                        .cleanup
                        .iter()
                        .filter(|action| matches!(action.outcome, preflight::CleanupOutcome::Failed(_)))
                        .count()
                ),
            });
        }
        // 预检干净（含清理已收敛）：遗留痕的善后已由预检复验代理 → 删除重建。
        if let Err(error) = store.delete(sid) {
            return Err(GateRejection::carrier(
                "trace_create_failed",
                format!("预检后删除遗留失败：{error:?}"),
            ));
        }
        store.create(sid, record).map_err(|error| {
            GateRejection::carrier(
                "trace_create_failed",
                format!("预检后重建失败：{error:?}"),
            )
        })?;
        log.emit(
            LogLevel::Info,
            "engine",
            "connect.gate.preflight.passed",
            "预检干净，遗留痕迹已清除并为本次连接重建",
            &[("elapsed_ms", &report.elapsed.as_millis().to_string())],
        );
        Ok(GateAdmission::RecreatedAfterPreflight)
    }

    /// 组装受理记录（connecting 态；server/命名空间尽力读取，校验留给组装段）。
    fn build_record(&self, envelope: &ConnectEnvelope) -> TraceRecord {
        let server = self
            .load_config()
            .map(|config| config.server)
            .unwrap_or_default();
        TraceRecord {
            schema: 1,
            connection_id: uuid::Uuid::new_v4().to_string(),
            state: TraceState::Connecting,
            engine_mode: self.engine_mode,
            kind: envelope.kind,
            server,
            // 现行产品形态：wintun TUN 承载隧道（系统代理族为路由 apply 的辅助步骤，
            // 无独立模式选择）——恒记 "tun"，模式域扩型时再随实况改写。
            mode: "tun".to_owned(),
            on_core_exit: envelope.on_core_exit,
            reconnect: envelope.reconnect.clone(),
            created_at_ms: unix_now_ms(),
            established_at_ms: 0,
            core_pid: envelope.core_pid,
            ui_pid: envelope.ui_pid,
            // 创建即记 engine PID（崩溃取证：装配中途崩溃时痕迹仍可归属）；
            // 装配后 mark_live 补丁同样携带。
            engine_pid: std::process::id(),
            adapter_names: Vec::new(),
            nrpt_owner_guids: Vec::new(),
            proxy_ports: Vec::new(),
        }
    }

    /// 发起用户 SID（写入前置校验）。
    fn user_sid_for_write(&self) -> Result<&str, GateRejection> {
        self.user_sid.as_deref().ok_or_else(|| {
            GateRejection::carrier(
                "trace_user_sid_missing",
                "发起用户 SID 缺失，无法落连接痕迹".to_owned(),
            )
        })
    }

    /// store（写入前置校验）。
    fn store_for_write(&self) -> Result<&ConnectionTraceStore, GateRejection> {
        self.store.as_ref().ok_or_else(|| {
            GateRejection::carrier(
                "trace_store_unavailable",
                "连接痕迹存储不可用（目录建立或加固失败）".to_owned(),
            )
        })
    }

    /// 读用户 config（尽力：预检命名空间 / 痕迹 server 字段）。
    fn load_config(&self) -> Option<exv_vpn_win32_config::ExvConfig> {
        load_config_at(&self.config_dir)
    }

    /// 校园命名空间清单（预检外来 NRPT 冲突匹配集）。
    fn campus_namespaces(&self) -> Vec<String> {
        campus_namespaces_in(&self.config_dir)
    }

    /// 痕迹写失败告警（不上抛）。
    fn warn_trace(&self, log: &LogSink, step: &str, error: &TraceStoreError) {
        log.emit(
            LogLevel::Warn,
            "engine",
            "connect.gate.trace.write_failed",
            "连接痕迹回写失败（不影响已建立的连接；残留由下次预检兜底）",
            &[("step", step), ("detail", format!("{error:?}").as_str())],
        );
    }

    // -----------------------------------------------------------------------
    // T5 善后路径的载体访问（删除权执行位；供 aftermath 驱动消费）
    // -----------------------------------------------------------------------

    /// 发起用户 SID（善后验证器构造预检输入用；无 SID/无 store = None）。
    #[must_use]
    pub fn user_sid_ref(&self) -> Option<&str> {
        self.user_sid.as_deref()
    }

    /// 校园命名空间清单（善后验证器的外来 NRPT 冲突匹配集，与预检路径同源）。
    #[must_use]
    pub fn campus_namespaces_ref(&self) -> Vec<String> {
        self.campus_namespaces()
    }

    /// 读当前痕（善后判定输入）。
    ///
    /// 返回 `None` = 载体不可用 / 缺 SID（已按 warn 记录）；`Some(outcome)` 原样
    /// 透传三态——`Corrupt` 是"善后未竟"的预检信号，调用方不得当作无痕处理。
    pub fn read_current_trace(&self, log: &LogSink) -> Option<TraceReadOutcome> {
        let Ok(sid) = self.user_sid_for_write() else {
            log.emit(
                LogLevel::Warn,
                "engine",
                "connect.gate.trace.read_no_sid",
                "善后路径读痕缺发起用户 SID（无痕保护会话，不驱动善后）",
                &[],
            );
            return None;
        };
        let Some(store) = self.store.as_ref() else {
            log.emit(
                LogLevel::Warn,
                "engine",
                "connect.gate.trace.read_no_store",
                "善后路径读痕时载体不可用（无痕保护会话，不驱动善后）",
                &[],
            );
            return None;
        };
        match store.read(sid) {
            Ok(outcome) => Some(outcome),
            Err(error) => {
                self.warn_trace(log, "read(善后)", &error);
                None
            }
        }
    }

    /// 痕 → Aftermath（善后收口路径；失败只告警——验证仍继续，残留由验证如实上报）。
    pub fn mark_aftermath(&self, log: &LogSink) {
        let Ok(sid) = self.user_sid_for_write() else {
            return;
        };
        let Some(store) = self.store.as_ref() else {
            return;
        };
        if let Err(error) = store.update_state(sid, TraceState::Aftermath) {
            self.warn_trace(log, "update_state(aftermath)", &error);
        }
    }

    /// 删痕（**只在善后验证通过后调用**，§2.4 删除权；返回是否删除成功）。
    pub fn delete_aftermath_trace(&self, log: &LogSink) -> bool {
        let Ok(sid) = self.user_sid_for_write() else {
            return false;
        };
        let Some(store) = self.store.as_ref() else {
            return false;
        };
        match store.delete(sid) {
            Ok(()) => true,
            Err(error) => {
                self.warn_trace(log, "delete(善后验证通过后)", &error);
                false
            }
        }
    }
}

/// Unix 毫秒时间戳（时钟回拨等异常退化为 0）。
fn unix_now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// 读用户 config（尽力：`<config_dir>/config.json`；缺省/不可解析 → None）。
fn load_config_at(config_dir: &Path) -> Option<exv_vpn_win32_config::ExvConfig> {
    let path = config_dir.join("config.json");
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 校园命名空间清单（预检外来 NRPT 冲突匹配集；读取失败按空集探测）。
fn campus_namespaces_in(config_dir: &Path) -> Vec<String> {
    load_config_at(config_dir)
        .map(|config| config.campus_dns_domains)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 单元测试：temp 目录上的真 store + 合成信封；预检分支不触真实系统（预检本体
// 的行为测试在 preflight.rs / classify.rs；全路径实机行为归 T13 验收门禁）。
// ---------------------------------------------------------------------------
