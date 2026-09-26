#pragma once

#include <string>

namespace exv::setup {

// True if current process token is elevated.
bool IsProcessElevated();

// Canonical string SID for the current process token user, or empty on failure.
std::wstring CurrentProcessUserSid();

// True if directory can be created/written by current user.
bool CanWriteDirectory(const std::wstring &directory);

// Launch self with /elevated-worker via ShellExecuteExW runas + SW_HIDE.
// Blocks until worker exits. Returns true if elevated process exited 0.
enum class ElevatedServiceOperation { Stop, Start };
bool RunElevatedWorker(const std::wstring &pipe_name, const std::wstring &token,
                       ElevatedServiceOperation operation = ElevatedServiceOperation::Stop,
                       std::wstring *error = nullptr);

// Stop an installed engine service while preserving its SCM registration.
// Uses the current token when elevated and otherwise a hidden runas worker.
bool EnsureServiceStopped();

// 恢复已有服务并确认 Running；非提升进程复用 token 认证的隐藏 worker。
bool EnsureServiceRunning(std::wstring *error = nullptr);

// Generate a simple one-time token (hex of random bytes).
std::wstring GenerateElevationToken();

// Default pipe name for this parent pid.
std::wstring DefaultElevationPipeName();

// Run elevated worker server side (called from ElevatedWorker role).
// Speaks a line protocol on the named pipe:
//   HELLO <token>
//   OP stop_service
//   OP start_service
//   OP quit
// Replies: OK / ERR <msg>
int RunElevatedWorkerServer(const std::wstring &pipe_name, const std::wstring &token);

}  // namespace exv::setup
