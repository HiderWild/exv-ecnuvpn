#pragma once

#include <cstdint>
#include <string>

namespace exv::setup {

inline constexpr wchar_t kEngineServiceName[] = L"exv-engine";

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


}  // namespace exv::setup
