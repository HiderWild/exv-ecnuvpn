
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::service::{
    install_service, scm_error, service_not_found, start_service, uninstall_service, SERVICE_NAME,
};

/// 批量协议版本（唯一接受 1）。
pub const BATCH_VERSION: u32 = 1;
/// 单次批量序列步数上限（防恶意超大序列占住提权进程）。
pub const MAX_BATCH_STEPS: usize = 8;
/// 请求体大小上限（64 KiB）。
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// Verify 步骤等待 SCM 进入 Running 的超时（Start 后服务先 StartPending，再 Running）。
pub const VERIFY_SCM_RUNNING_TIMEOUT: Duration = Duration::from_secs(30);
/// SCM Running 轮询间隔。
const SCM_POLL_INTERVAL: Duration = Duration::from_millis(250);

// ---------------------------------------------------------------------------
// Wire 类型（serde JSON；`deny_unknown_fields`——host 侧旧字段/拼写错误在解析即拒）。
// ---------------------------------------------------------------------------

/// 一次批量提权请求（host 写入 `<req-file>`；engine 读取执行）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceBatchRequest {
    /// 协议版本（必须等于 [`BATCH_VERSION`]）。
    pub version: u32,
    /// 顺序执行的步骤序列（≤ [`MAX_BATCH_STEPS`]；无 Stop——停止线已在阶段 1 移除）。
    pub sequence: Vec<BatchStep>,
    /// 用户配置目录（绝对路径；Install 步骤注册进服务启动参数）。
    pub config_dir: String,
}

/// 批量步骤（**固定枚举**，无任意命令执行入口）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BatchStep {
    /// SCM 安装（含 REPAIR 语义，复用 `install_service`）。
    Install,
    /// SCM 启动。
    Start,
    /// SCM 卸载（内部先停，复用 `uninstall_service`）。
    Uninstall,
    /// 检查 SCM Running（轮询等待服务进入 Running）。
    Verify,
    /// 检查 SCM 服务已不存在。
    VerifyRemoved,
    /// 轮换（撤销）服务 PSK（2026-09-05 撤销计划：`write_service_psk` 覆盖写新密钥
    /// + DACL 重建；无需重启服务，撤销自下一次连接起生效）。
    RotateKey,
}

/// 单步执行结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BatchStepResult {
    /// 该步对应的步骤。
    pub step: BatchStep,
    /// 是否成功。
    pub ok: bool,
    /// 成功 = 描述；失败 = 携带原因。
    pub message: String,
}

/// 批量执行结果（engine 写入 `<result-file>`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServiceBatchResult {
    /// 全部步骤成功。
    pub ok: bool,
    /// 逐步骤结果（失败即中止；仅含已执行步骤）。
    pub steps: Vec<BatchStepResult>,
    /// 总述（成功 = "batch completed"；失败 = 定位到失败步骤）。
    pub message: String,
}

// ---------------------------------------------------------------------------
// 校验（纯逻辑，可单测）
// ---------------------------------------------------------------------------

/// 校验请求形状：version == 1、序列 ≤ [`MAX_BATCH_STEPS`]、`config_dir` 绝对路径。
///
/// # Errors
/// 任一校验不通过 → 携带原因的字符串（fail closed）。
pub fn validate_request(request: &ServiceBatchRequest) -> Result<(), String> {
    if request.version != BATCH_VERSION {
        return Err(format!(
            "unsupported batch version {} (expected {BATCH_VERSION})",
            request.version
        ));
    }
    if request.sequence.len() > MAX_BATCH_STEPS {
        return Err(format!(
            "batch sequence of {} steps exceeds limit {MAX_BATCH_STEPS}",
            request.sequence.len()
        ));
    }
    let config = PathBuf::from(&request.config_dir);
    if request.config_dir.is_empty() || !config.is_absolute() {
        return Err("batch config_dir must be an absolute directory path".to_string());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 服务操作 seam（测试注入 fake；生产 = [`RealServiceOps`]）
// ---------------------------------------------------------------------------

/// 批量步骤对应的服务操作原语（可注入 seam——单测不触碰真 SCM/管道）。
///
/// 每个方法返回 `Err` 表示该步骤失败（携带原因；编排层随即中止后续步骤）。
pub trait BatchServiceOps {
    /// `Install`：SCM 安装（含 REPAIR）。
    ///
    /// # Errors
    /// SCM 安装 / 修复失败 → 携带原因的字符串。
    fn install(&mut self, config_dir: &str) -> Result<(), String>;
    /// `Start`：SCM 启动。
    ///
    /// # Errors
    /// SCM 启动失败（含服务未安装）→ 携带原因的字符串。
    fn start(&mut self) -> Result<(), String>;
    /// `Uninstall`：SCM 卸载（内部先停）。
    ///
    /// # Errors
    /// SCM 卸载 / 残留清理失败 → 携带原因的字符串。
    fn uninstall(&mut self) -> Result<(), String>;
    /// `Verify`：SCM Running + keepalive 探活。
    ///
    /// # Errors
    /// SCM 非 Running / 探活失败 / 超时 → 携带原因的字符串。
    fn verify(&mut self) -> Result<(), String>;
    /// `VerifyRemoved`：SCM 服务已不存在。
    ///
    /// # Errors
    /// 服务仍注册 / SCM 查询失败 → 携带原因的字符串。
    fn verify_removed(&mut self) -> Result<(), String>;
    /// `RotateKey`：轮换（撤销）服务 PSK（覆盖写新密钥）。
    ///
    /// 成功 = 返回新 key 的 fingerprint（`hex(SHA256(psk)[0..8])`，16 hex，非秘密；
    /// 编排层组装为 `rotated fingerprint=<16hex>` 步骤消息，host 据此提取）。
    ///
    /// # Errors
    /// 当前用户 SID 不可得 / PSK 生成或写入失败（ACL 拒绝等）→ 携带原因的字符串。
    fn rotate_key(&mut self) -> Result<String, String>;
}

/// 生产服务操作：复用 `service.rs` 既有 SCM 原语（Verify 走 SCM Running 轮询）。
struct RealServiceOps;

impl BatchServiceOps for RealServiceOps {
    fn install(&mut self, config_dir: &str) -> Result<(), String> {
        // 复用 `install_service`：解析参数从 argv 读；`--config-dir` 显式传入，其余
        // 回退默认（dll 冻结路径 / adapter 默认名 / 当前用户 SID——runas 提权保持同用户）。
        let argv = vec![
            "exv-engine".to_string(),
            "--service-install".to_string(),
            "--config-dir".to_string(),
            config_dir.to_string(),
        ];
        install_service(&argv)
    }

    fn start(&mut self) -> Result<(), String> {
        start_service()
    }

    fn uninstall(&mut self) -> Result<(), String> {
        uninstall_service()
    }

    fn verify(&mut self) -> Result<(), String> {
        verify_service_ready()
    }

    fn verify_removed(&mut self) -> Result<(), String> {
        verify_service_removed()
    }

    fn rotate_key(&mut self) -> Result<String, String> {
        // 复用 CLI/批量共用的轮换原语（write_service_psk(current_user_sid) + fingerprint）。
        crate::service::rotate_service_key()
    }
}

// ---------------------------------------------------------------------------
// 编排：顺序执行 + 任一步失败即中止（可注入 ops；单测走 fake）
// ---------------------------------------------------------------------------

/// 执行请求的完整序列并返回结果结构（不写文件；校验失败同样产出 ok=false 结果）。
fn execute_batch(request: &ServiceBatchRequest, ops: &mut dyn BatchServiceOps) -> ServiceBatchResult {
    if let Err(e) = validate_request(request) {
        return ServiceBatchResult {
            ok: false,
            steps: Vec::new(),
            message: e,
        };
    }
    let t_batch = Instant::now();
    let mut steps = Vec::with_capacity(request.sequence.len());
    for (index, step) in request.sequence.iter().enumerate() {
        let t_step = Instant::now();
        // 成功消息：RotateKey 携带新 key 的 fingerprint（4.4 审计契约——host 从该消息
        // 提取并组装冻结回复文案，与 engine accept 日志可关联；禁 PSK 原文/HMAC），
        // 其余步骤沿用稳定标签。
        let outcome: Result<String, String> = match step {
            BatchStep::Install => {
                ops.install(&request.config_dir).map(|()| step_label(*step).to_string())
            }
            BatchStep::Start => ops.start().map(|()| step_label(*step).to_string()),
            BatchStep::Uninstall => ops.uninstall().map(|()| step_label(*step).to_string()),
            BatchStep::Verify => ops.verify().map(|()| step_label(*step).to_string()),
            BatchStep::VerifyRemoved => {
                ops.verify_removed().map(|()| step_label(*step).to_string())
            }
            BatchStep::RotateKey => ops
                .rotate_key()
                .map(|fingerprint| format!("rotated fingerprint={fingerprint}")),
        };
        let step_elapsed = t_step.elapsed().as_millis();
        eprintln!("[exv-batch] step {} ({}): ok={} elapsed={}ms", index, step_label(*step), outcome.is_ok(), step_elapsed);
        match outcome {
            Ok(message) => {
                steps.push(BatchStepResult {
                    step: *step,
                    ok: true,
                    message,
                });
            }
            Err(e) => {
                steps.push(BatchStepResult {
                    step: *step,
                    ok: false,
                    message: e.clone(),
                });
                return ServiceBatchResult {
                    ok: false,
                    steps,
                    message: format!("step {} ({}) failed: {e}", index + 1, step_label(*step)),
                };
            }
        }
    }
    let total_elapsed_ms = t_batch.elapsed().as_millis();
    eprintln!("[exv-batch] all steps completed: {} steps in {}ms", request.sequence.len(), total_elapsed_ms);
    ServiceBatchResult {
        ok: true,
        steps,
        message: "batch completed".to_string(),
    }
}

/// 步骤的稳定标签（结果消息 / 失败定位用）。
fn step_label(step: BatchStep) -> &'static str {
    match step {
        BatchStep::Install => "install",
        BatchStep::Start => "start",
        BatchStep::Uninstall => "uninstall",
        BatchStep::Verify => "verify",
        BatchStep::VerifyRemoved => "verify_removed",
        BatchStep::RotateKey => "rotate_key",
    }
}

/// 执行批量并把 [`ServiceBatchResult`] 写入 `<result-path>`。
///
/// 任一校验失败 / 任一步失败 → result ok=false（仍写入），返回 `Err`（main 以非零码退）。
///
/// # Errors
/// 校验失败 / 任一步失败 / 结果文件写入失败 → 携带原因的字符串。
pub fn run_batch(request: &ServiceBatchRequest, result_path: &Path) -> Result<(), String> {
    run_batch_with_ops(request, result_path, &mut RealServiceOps)
}

/// [`run_batch`] 的可注入 ops 变体（单测用 fake ops，避免真 SCM/管道依赖）。
fn run_batch_with_ops(
    request: &ServiceBatchRequest,
    result_path: &Path,
    ops: &mut dyn BatchServiceOps,
) -> Result<(), String> {
    let result = execute_batch(request, ops);
    write_result(result_path, &result)?;
    if result.ok {
        Ok(())
    } else {
        Err(result.message)
    }
}

/// 把结果 JSON 写入 `<result-path>`（父目录由 host 保证存在）。
///
/// # Errors
/// 序列化 / 写文件失败 → 携带原因的字符串。
fn write_result(result_path: &Path, result: &ServiceBatchResult) -> Result<(), String> {
    let json = serde_json::to_string_pretty(result)
        .map_err(|e| format!("serialize batch result: {e}"))?;
    std::fs::write(result_path, json)
        .map_err(|e| format!("write batch result {}: {e}", result_path.display()))
}

/// 写一个仅含 message 的失败结果（请求解析/超限等执行前失败也要让 host 可诊断）。
fn write_error_result(result_path: &Path, message: &str) -> Result<(), String> {
    write_result(
        result_path,
        &ServiceBatchResult {
            ok: false,
            steps: Vec::new(),
            message: message.to_string(),
        },
    )
}

// ---------------------------------------------------------------------------
// 文件生命周期：读 req（≤64KB）→ 执行 → 写 result → finally 删除 req；result 保留给 host（读后删）。
// ---------------------------------------------------------------------------

/// 从 `<request-path>` 读请求（校验 ≤ [`MAX_REQUEST_BYTES`]）、执行并写结果到
/// `<result-path>`。**finally 删除 req 文件**（输入已消费）；**result 文件保留**，
/// 由 host 在 `wait_exit_code` 后读取并校验，读毕删除。
///
/// # Errors
/// 请求读取 / 超限 / 解析失败，或批量执行失败 / 结果写入失败 → 携带原因的字符串。
pub fn run_batch_from_files(request_path: &Path, result_path: &Path) -> Result<(), String> {
    run_batch_from_files_with_ops(request_path, result_path, &mut RealServiceOps)
}

/// [`run_batch_from_files`] 的可注入 ops 变体（单测走 fake ops）。
fn run_batch_from_files_with_ops(
    request_path: &Path,
    result_path: &Path,
    ops: &mut dyn BatchServiceOps,
) -> Result<(), String> {
    // finally 清理：无论成功/失败/异常都删除 req（输入已消费）。result 是向 host 交付
    // 结果的唯一通道（ShellExecuteExW 无 stdio），必须保留到 host 读取后由 host 删除。
    struct Cleanup<'a> {
        request: &'a Path,
    }
    impl Drop for Cleanup<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.request);
        }
    }
    let _cleanup = Cleanup {
        request: request_path,
    };

    let bytes = std::fs::read(request_path)
        .map_err(|e| format!("read batch request {}: {e}", request_path.display()))?;
    if bytes.len() > MAX_REQUEST_BYTES {
        let message = format!("batch request exceeds {MAX_REQUEST_BYTES} bytes");
        write_error_result(result_path, &message)?;
        return Err(message);
    }
    let request: ServiceBatchRequest = match serde_json::from_slice(&bytes) {
        Ok(request) => request,
        Err(e) => {
            let message = format!("batch request malformed: {e}");
            write_error_result(result_path, &message)?;
            return Err(message);
        }
    };
    run_batch_with_ops(&request, result_path, ops)
}

// ---------------------------------------------------------------------------
// S2-C 孤儿 watchdog：批量进程运行期间监控 host；host 中途退出 → 终止（防孤儿）。
// 决策/主体在 lib（可单测 seam）；进程等待由 main 注入（复用 `wait_core_process_exit`
// 的同一实现），生产终止 = `process::exit`。
// ---------------------------------------------------------------------------

/// 批量孤儿 watchdog 决策（可测）：host 退出信号已到，判断是否应终止批量进程。
///
/// - `completed == true`（批量已返回，result 已写/已交付）→ **不终止**——进程即将正常
///   退出，watchdog 恰在此时返回不得误杀正常路径。
/// - `completed == false`（host 中途退出、批量未完成）→ **终止**——防孤儿。
pub fn should_terminate_orphan_batch(completed: &AtomicBool) -> bool {
    !completed.load(Ordering::SeqCst)
}

/// 批量孤儿 watchdog 主体（可测 seam）：等待 host 退出信号；批量未完成 → 终止。
///
/// 生产路径由 main `spawn_batch_orphan_watchdog` 注入真实进程等待
/// （`wait_core_process_exit` 的同步实现）+ `process::exit`；单测注入 fake 等待 /
/// 记录式终止验证「已完成不杀 / 未完成杀」决策与竞态处理。
pub fn orphan_watchdog_body(
    wait_host_exit: impl FnOnce(),
    completed: &AtomicBool,
    terminate: impl FnOnce(),
) {
    wait_host_exit();
    if should_terminate_orphan_batch(completed) {
        terminate();
    }
}

// ---------------------------------------------------------------------------
// 生产 Verify / VerifyRemoved：SCM 查询（Verify 只确认 Running，不做管道探活）。
// ---------------------------------------------------------------------------

/// `Verify`：SCM Running 轮询等待（Start 后服务经 StartPending→Running）。
///
/// 注意：批量进程是 runas 提权的**用户**身份，而服务控制管道 DACL = SYSTEM + core_sid
/// （WSP1 §4 frozen shape），用户进程打开管道被拒 → 原 keepalive 管道探活必然
/// 「transport error」（2026-08-23 实测）。控制面 liveness 由 host 的 connect 完整
/// 校验（core SID 在 DACL 内 + PSK 握手），批量 Verify 只确认 SCM Running 即可。
fn verify_service_ready() -> Result<(), String> {
    wait_for_service_running(VERIFY_SCM_RUNNING_TIMEOUT)
}

/// 轮询等待 SCM 服务进入 Running（`StartPending`/`StopPending` 等过渡态不判失败）。
fn wait_for_service_running(timeout: Duration) -> Result<(), String> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match service_scm_running() {
            Ok(()) => return Ok(()),
            Err(e) => {
                if std::time::Instant::now() >= deadline {
                    return Err(e);
                }
                std::thread::sleep(SCM_POLL_INTERVAL);
            }
        }
    }
}

/// `VerifyRemoved`：SCM 服务已不存在（`ERROR_SERVICE_DOES_NOT_EXIST` = 1060）。
fn verify_service_removed() -> Result<(), String> {
    use windows_service::service::ServiceAccess;
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(scm_error)?;
    match manager.open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS) {
        Ok(_) => Err("service still registered".to_string()),
        Err(e) if service_not_found(&e) => Ok(()),
        Err(e) => Err(scm_error(e)),
    }
}

/// SCM 状态必须为 Running。
fn service_scm_running() -> Result<(), String> {
    use windows_service::service::{ServiceAccess, ServiceState};
    use windows_service::service_manager::{ServiceManager, ServiceManagerAccess};

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .map_err(scm_error)?;
    let service = manager
        .open_service(SERVICE_NAME, ServiceAccess::QUERY_STATUS)
        .map_err(scm_error)?;
    let status = service.query_status().map_err(scm_error)?;
    if status.current_state != ServiceState::Running {
        return Err(format!(
            "service not running (state={:?})",
            status.current_state
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// 单元测试：校验 / 编排 / 文件生命周期（fake ops，不触碰真 SCM/管道）。
// SCM 真机集成（安装/启动/卸载/探活全链）留 S2-C / 业务验收（env 门控 + 标 ignored）。
// ---------------------------------------------------------------------------
