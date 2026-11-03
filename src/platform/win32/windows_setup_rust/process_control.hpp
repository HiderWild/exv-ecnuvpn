#pragma once

#include <cstdint>
#include <string>
#include <utility>
#include <vector>

namespace exv::setup {

// 进程处置结果（2026-10-10 修复计划 T3 / F5）：杀不掉的 EXV 进程名单。
// OpenProcess 失败的进程（典型为其他用户会话：asInvoker 安装器无跨会话
// 权限）无法在杀进程阶段终止，解压失败时多半正是它们仍占用安装树文件；
// 上层须把名单如实并入失败文案，不再静默。
struct AppProcessStopReport {
  // 每项形如 L"exv-ui.exe (PID 1234)"。
  std::vector<std::wstring> inaccessible_processes;
};

// 注入缝（2026-10-10 修复计划 T3 步骤 0）：进程枚举/打开/终止/等待经此
// 函数表路由，默认指向 Win32 实现（定义在 process_control.cpp）。测试在
// 运行前替换字段即可注入确定性进程视图、假等待（快进 15s 自退预算）或
// 失败注入；既有「链接期整函数替换 StopRunningAppProcesses」的测试手法
// 保持兼容（该符号仍只定义一次）。
// 句柄以 void* 透传（头文件不引入 windows.h）；wait_for_exit 返回
// WAIT_OBJECT_0(0) / WAIT_TIMEOUT(258) 语义值；enumerate_processes 返回
// 的映像名必须已是小写（与 Win32 默认实现一致）。
struct ProcessControlOps {
  std::vector<std::pair<std::uint32_t, std::wstring>> (*enumerate_processes)();
  void *(*open_process)(std::uint32_t pid);
  bool (*terminate_process)(void *process);
  std::uint32_t (*wait_for_exit)(void *process, std::uint32_t timeout_ms);
  void (*sleep)(std::uint32_t milliseconds);
  void (*close_handle)(void *handle);
};

extern ProcessControlOps g_process_control_ops;

// 形态感知杀序（2026-10-10 修复计划 T3 步骤 2）：exv-ui → exv-core → 按
// exv-engine 形态分叉——SCM 有 exv-engine 服务（服务形态）时 engine 不进
// 终止名单，留给调用链既有的 SCM 停服/移除路径做优雅 teardown（安装器已
// 先停服、卸载器下一步即服务移除）；无服务（oneshot 形态）时等引擎自退
// （等待预算见 process_control.cpp 的 kEngineSelfExitWaitMs，对齐引擎侧
// 5s 组装取消 + 8s NRPT 清理 + 300ms flush），超时才 TerminateProcess 兜底。
// core 先死使引擎 respawn 无源。返回杀不掉的进程名单（F5）。
AppProcessStopReport StopRunningAppProcesses();

// 只探测不结束：是否有任一 EXV 进程（exv-ui/core/engine）正在运行。
// 安装前检查旧版本实例时使用，避免「无进程也要走一遍结束流程」。
bool IsAnyExvProcessRunning();

// Terminate all processes whose image name matches (case-insensitive).
// Returns number of terminate attempts.
int TerminateProcessesByImageName(const std::wstring &image_name);

}  // namespace exv::setup
