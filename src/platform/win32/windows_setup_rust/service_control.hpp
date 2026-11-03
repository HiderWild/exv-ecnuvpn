#pragma once

#include <cstdint>
#include <string>

namespace exv::setup {

inline constexpr wchar_t kEngineServiceName[] = L"exv-engine";

// KI-13 ⑥ 历代服务漂移：C++ 3.3.x 时代的提升助手 SCM 服务名（NSIS
// !define SERVICE_NAME "exv-helper"，payload 在 %ProgramData%\EXV\Helper）。
// 一路覆盖升级从未卸载的宿主可能仍注册着该遗留服务，卸载时需要一并移除。
inline constexpr wchar_t kLegacyHelperServiceName[] = L"exv-helper";

enum class ServicePresence {
  Installed,
  NotInstalled,
  Error,
};

ServicePresence QueryServicePresence(const std::wstring &service_name = kEngineServiceName);

bool IsServiceInstalled(const std::wstring &service_name = kEngineServiceName);

enum class ServiceRuntimeState { NotInstalled, Stopped, Running, Other, Error };

struct ServiceRuntimeSnapshot {
  ServiceRuntimeState state = ServiceRuntimeState::Error;
  std::uint32_t scm_state = 0;
  std::uint32_t error_code = 0;
};

// 精确读取停服前状态；过渡态保留 SCM 原值，不推断恢复意图。
ServiceRuntimeSnapshot QueryServiceRuntimeState(
    const std::wstring &service_name = kEngineServiceName);

// 只启动已有服务并有界等待 Running；缺失服务明确失败，绝不创建。
bool StartServiceAndWait(const std::wstring &service_name = kEngineServiceName,
                         std::wstring *error = nullptr);

// Stop the service without deleting its SCM registration. Idempotent when the
// service is absent or already stopped.
bool StopService(const std::wstring &service_name = kEngineServiceName);

// Stop then delete service via SCM. Requires sufficient privileges.
// Returns true if service is gone (or was never installed).
bool StopAndDeleteService(const std::wstring &service_name = kEngineServiceName);

// Poll until service is not installed or attempts exhausted.
bool WaitForServiceRemoved(const std::wstring &service_name = kEngineServiceName,
                           int max_attempts = 10,
                           int poll_ms = 200);

// Rust line: run "exv-engine.exe" --service-uninstall (self-contained removal of
// the Rust SCM service exv-engine + PSK).
bool RunEngineServiceUninstall(const std::wstring &engine_exe_path);

// ---- S5（2026-10-10 换盘服务迁移，计划 §六 v2）----
// 覆盖安装只停服不改 ImagePath，换盘安装后服务仍指向旧目录二进制（可能已被
// 用户整目录搬走/删除）。停服前经 QueryServiceConfigW 读 ImagePath 判漂移，
// 漂移时经提权会话 migrate_service OP 改写（ChangeServiceConfigW 需
// SERVICE_CHANGE_CONFIG——默认服务 SD 只授 Administrators/SYSTEM，承载见
// elevate.hpp）。引擎注册的 ImagePath 形态：可执行段（按需引号）+ 尾参
// --service --control-pipe <sid 派生管道名> --dll <冻结 wintun 路径>
// --adapter-name --user-sid --config-dir（exv-vpn-win32-engine service.rs
// service_launch_arguments）——尾参全部与安装目录无关，换盘漂移只在可执行段。

// ImagePath 可执行段/尾参切分结果（纯函数，无 SCM 访问）。
struct ServiceImagePathParts {
  std::wstring executable;  // 去引号的可执行段（未引号含空格路径按首段空白前截断）
  std::wstring arguments;   // 尾参原文（去前导空白，内部引号原样保留）
};

// 切分 ImagePath：引号内整体为可执行段（未闭合容错取引号后全部）；未加引号时
// 取首段空白前。比对与迁移都先切分——只比较/只改写可执行段，尾参原样保留。
ServiceImagePathParts SplitServiceImagePathExecutable(const std::wstring &image_path);

// 路径形态规整（纯函数）：去首尾空白与包围引号，斜杠统一反斜杠、大小写规整
// 小写（NTFS 大小写不敏感；非 ASCII 的大小写等价不保证——误判方向只会多一次
// 幂等改写，安全）。仅规整形态、不切分尾参：完整 ImagePath 必须先经
// SplitServiceImagePathExecutable 取可执行段再规整（未引号含空格路径会被切分
// 截断，裸路径直接规整安全）；仅用于比对，不改写 SCM 内值。
std::wstring NormalizeServiceImagePathForCompare(const std::wstring &path);

// 换盘漂移判定（纯函数）：ImagePath 的可执行段（切分去引号/尾参）与预期裸
// 可执行路径各自规整后比较——安装侧判定 ImagePath 是否指向本次安装目录。
bool ServiceImagePathDrifted(const std::wstring &image_path,
                             const std::wstring &expected_engine_path);

// QueryServiceConfigW 读取服务 ImagePath 的三态快照（SERVICE_QUERY_CONFIG，
// 非提权进程可读）。binary_path 为 lpBinaryPathName 原文（含引号/尾参形态，
// 错误文案与尾参保真用）。
struct ServiceImagePathSnapshot {
  ServicePresence presence = ServicePresence::Error;
  std::wstring binary_path;
  std::uint32_t error_code = 0;
};

ServiceImagePathSnapshot QueryServiceImagePath(
    const std::wstring &service_name = kEngineServiceName);

// ChangeServiceConfigW 改写 ImagePath 可执行段至 new_engine_path，尾参（控制
// 面管道名/dll/SID/config-dir）原样保留——这些是安装目录无关的按用户派生值，
// 改写丢失会让服务启动退化到默认参数。写入带引号惯例形态（CreateService 同
// 惯例），写后回读校验；失败置 error（ASCII Win32 诊断）并返回 false，
// fail-closed 拒装判定在调用侧。
bool MigrateServiceImagePath(const std::wstring &new_engine_path,
                             const std::wstring &service_name = kEngineServiceName,
                             std::wstring *error = nullptr);


}  // namespace exv::setup
