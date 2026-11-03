#include "windows_setup_rust/process_control.hpp"

#include "windows_setup_rust/service_control.hpp"
#include "windows_setup_rust/util/hidden_process.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <tlhelp32.h>

#include <algorithm>
#include <cstring>
#include <string>
#include <vector>

namespace exv::setup {
namespace {

constexpr wchar_t kUiImageName[] = L"exv-ui.exe";
constexpr wchar_t kCoreImageName[] = L"exv-core.exe";
constexpr wchar_t kEngineImageName[] = L"exv-engine.exe";

std::wstring ToLower(std::wstring v) {
  for (auto &ch : v) {
    if (ch >= L'A' && ch <= L'Z') {
      ch = static_cast<wchar_t>(ch - L'A' + L'a');
    }
  }
  return v;
}

// ---- 注入缝的默认 Win32 实现（2026-10-10 修复计划 T3 步骤 0）----
// 生产路径只经 g_process_control_ops 调用这些函数；测试替换表字段即可。

std::vector<std::pair<std::uint32_t, std::wstring>> Win32EnumerateProcesses() {
  std::vector<std::pair<std::uint32_t, std::wstring>> entries;
  HANDLE snap = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0);
  if (snap == INVALID_HANDLE_VALUE) {
    return entries;
  }
  PROCESSENTRY32W pe{};
  pe.dwSize = sizeof(pe);
  if (Process32FirstW(snap, &pe)) {
    do {
      entries.emplace_back(pe.th32ProcessID, ToLower(pe.szExeFile));
    } while (Process32NextW(snap, &pe));
  }
  CloseHandle(snap);
  return entries;
}

void *Win32OpenProcess(std::uint32_t pid) {
  return OpenProcess(PROCESS_TERMINATE | SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                     FALSE, pid);
}

bool Win32TerminateProcess(void *process) {
  return TerminateProcess(static_cast<HANDLE>(process), 1) != FALSE;
}

std::uint32_t Win32WaitForExit(void *process, std::uint32_t timeout_ms) {
  return WaitForSingleObject(static_cast<HANDLE>(process), timeout_ms);
}

void Win32Sleep(std::uint32_t milliseconds) { Sleep(milliseconds); }

void Win32CloseHandle(void *handle) { CloseHandle(static_cast<HANDLE>(handle)); }

// oneshot 引擎自退等待预算（2026-10-10 修复计划 T3，复审 M1）：对齐引擎侧
// 善后链各段上限——在途组装取消 5s（engine heartbeat.rs 的
// ASSEMBLY_CANCEL_WAIT）+ NRPT/系统代理/网卡清理验证上限 8s（engine
// aftermath.rs 的 VERIFY_BOUND）+ 状态 flush 300ms（engine heartbeat.rs：
// teardown → post Idle → flush → 退）+ 调度余量 ⇒ 15s。core 已死是前提：
// 引擎善后由 core 死亡驱动，杀 core 在前、等 engine 在后。
constexpr std::uint32_t kEngineSelfExitWaitMs = 15000;

// 终止指定映像的全部实例，经注入缝路由；OpenProcess 失败的进程并入
// 他用户名单（F5）。返回 TerminateProcess 成功数（与旧
// TerminateProcessesByImageName 同口径）。
int TerminateImageInstances(const wchar_t *image_name, AppProcessStopReport *report) {
  const auto &ops = g_process_control_ops;
  const std::wstring target = ToLower(image_name);
  int attempts = 0;
  for (const auto &entry : ops.enumerate_processes()) {
    if (entry.second != target) {
      continue;
    }
    void *process = ops.open_process(entry.first);
    if (process == nullptr) {
      // F5：多为其他用户会话的进程（asInvoker 无跨会话权限）。枚举→打开
      // 间隙内自退的进程理论上可能误入名单；该名单只用于解压失败时的提示
      // 文案，不参与控制流，误入不改变行为。
      if (report != nullptr) {
        report->inaccessible_processes.push_back(target + L" (PID " +
                                                 std::to_wstring(entry.first) + L")");
      }
      continue;
    }
    if (ops.terminate_process(process)) {
      ++attempts;
    }
    ops.wait_for_exit(process, 2000);
    ops.close_handle(process);
  }
  return attempts;
}

// oneshot 形态：等引擎自退；等不到（WAIT_TIMEOUT/WAIT_FAILED，预算已尽或
// 句柄不可等）才 TerminateProcess 兜底。返回是否所有实例都已确认退出
// （自退成功，或强杀后 2s 内等回）。
bool StopOneshotEngineInstances(AppProcessStopReport &report) {
  const auto &ops = g_process_control_ops;
  const std::wstring target = ToLower(kEngineImageName);
  bool all_exited = true;
  for (const auto &entry : ops.enumerate_processes()) {
    if (entry.second != target) {
      continue;
    }
    void *process = ops.open_process(entry.first);
    if (process == nullptr) {
      report.inaccessible_processes.push_back(target + L" (PID " +
                                              std::to_wstring(entry.first) + L")");
      all_exited = false;
      continue;
    }
    if (ops.wait_for_exit(process, kEngineSelfExitWaitMs) == WAIT_OBJECT_0) {
      // 自退成功：引擎善后（取消在途组装 → NRPT/系统代理/网卡 teardown →
      // flush）已在预算内完成，零残留，无需强杀。
      ops.close_handle(process);
      continue;
    }
    ops.terminate_process(process);
    if (ops.wait_for_exit(process, 2000) != WAIT_OBJECT_0) {
      all_exited = false;
    }
    ops.close_handle(process);
  }
  return all_exited;
}

}  // namespace

// 注入缝默认表：生产路径即 Win32；测试在运行前按需替换字段。
ProcessControlOps g_process_control_ops = {
    Win32EnumerateProcesses, Win32OpenProcess,     Win32TerminateProcess,
    Win32WaitForExit,        Win32Sleep, Win32CloseHandle};

bool IsAnyExvProcessRunning() {
  for (const auto &entry : g_process_control_ops.enumerate_processes()) {
    if (entry.second == kUiImageName || entry.second == kCoreImageName ||
        entry.second == kEngineImageName) {
      return true;
    }
  }
  return false;
}

int TerminateProcessesByImageName(const std::wstring &image_name) {
  return TerminateImageInstances(image_name.c_str(), nullptr);
}

AppProcessStopReport StopRunningAppProcesses() {
  // 2026-10-10 修复计划 T3 步骤 2（复审 M1/M2）：形态感知杀序。
  // ① exv-ui ② exv-core：core 先死使引擎失去 respawn 源——core 存活期停服
  //    会触发 P3 respawn 提权重拉（反噬），core 先死后 respawn 无源。
  // ③ exv-engine 按形态分叉：
  //    - SCM 有 exv-engine 服务（服务形态）：不进终止名单。优雅 teardown 留给
  //      调用链既有的 SCM 路径——安装器在杀进程前已 EnsureServiceStopped，
  //      卸载器下一步即服务移除块（保持在本次调用之后，见 uninstall_engine.cpp）。
  //    - 无服务（oneshot 形态）：等引擎自退（预算 kEngineSelfExitWaitMs），
  //      超时才 TerminateProcess 兜底。
  // C++ 时代的命名管道优雅停机（见 2026-10-10 修复计划 F3）是恒不可达的死
  // 代码，已删除；Rust 线的实际控制面是按 UI PID 命名的 gRPC 鉴权管道，与
  // 旧 JSON 行协议不兼容，安装器不再尝试。
  AppProcessStopReport report;
  TerminateImageInstances(kUiImageName, &report);
  TerminateImageInstances(kCoreImageName, &report);

  const bool engine_owned_by_scm =
      QueryServicePresence(kEngineServiceName) == ServicePresence::Installed;
  bool engine_all_exited = true;
  if (!engine_owned_by_scm) {
    engine_all_exited = StopOneshotEngineInstances(report);
  }

  g_process_control_ops.sleep(300);
  // Hidden fallbacks matching NSIS taskkill behavior（残余兜底）。服务形态下
  // engine 归调用链的 SCM 路径处置，绝不在此 taskkill；oneshot 形态仅在
  // 自退等待未确认全部退出时才对 engine 兜底。
  TaskKillImage(kUiImageName);
  TaskKillImage(kCoreImageName);
  if (!engine_owned_by_scm && !engine_all_exited) {
    TaskKillImage(kEngineImageName);
  }
  return report;
}

}  // namespace exv::setup
