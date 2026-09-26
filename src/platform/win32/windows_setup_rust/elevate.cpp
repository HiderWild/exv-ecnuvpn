#include "windows_setup_rust/elevate.hpp"

#include "windows_setup_rust/service_control.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shellapi.h>
#include <sddl.h>

#include <cstdio>
#include <cstring>
#include <filesystem>
#include <string>
#include <cstdint>
#include <vector>

namespace exv::setup {
namespace {

std::wstring GetSelfPath() {
  wchar_t path[MAX_PATH] = {};
  const DWORD n = GetModuleFileNameW(nullptr, path, MAX_PATH);
  if (n == 0 || n >= MAX_PATH) {
    return {};
  }
  return path;
}

// 每次调用独占 OVERLAPPED/event；返回前必须收回取消完成，不能让内核继续引用栈内存。
class PipeOperation {
 public:
  PipeOperation() { overlapped.hEvent = CreateEventW(nullptr, TRUE, FALSE, nullptr); }
  ~PipeOperation() { if (overlapped.hEvent) CloseHandle(overlapped.hEvent); }
  PipeOperation(const PipeOperation &) = delete;
  PipeOperation &operator=(const PipeOperation &) = delete;
  OVERLAPPED overlapped{};
};

bool CompletePipeOperation(HANDLE pipe, PipeOperation &operation, BOOL immediate,
                           ULONGLONG deadline, HANDLE peer, DWORD &transferred) {
  if (!immediate && GetLastError() != ERROR_IO_PENDING) {
    return false;
  }
  auto &overlapped = operation.overlapped;
  if (!immediate) {
    const ULONGLONG now = GetTickCount64();
    const DWORD remaining = now >= deadline ? 0 : static_cast<DWORD>(deadline - now);
    const HANDLE waits[] = {overlapped.hEvent, peer};
    const DWORD wait = WaitForMultipleObjects(peer ? 2 : 1, waits, FALSE, remaining);
    if (wait != WAIT_OBJECT_0) {
      const DWORD error = wait == WAIT_TIMEOUT ? ERROR_TIMEOUT
                          : wait == WAIT_OBJECT_0 + 1 ? ERROR_PROCESS_ABORTED : GetLastError();
      // 本地命名管道支持 CancelIoEx。取消后收回完成只等内核取消，不再等对端读写；
      // 即使完成恰好抢先发生，也必须读取完成结果后才能释放 OVERLAPPED 和缓冲区。
      CancelIoEx(pipe, &overlapped);
      GetOverlappedResult(pipe, &overlapped, &transferred, TRUE);
      SetLastError(error);
      return false;
    }
  }
  return GetOverlappedResult(pipe, &overlapped, &transferred, FALSE) != FALSE;
}

bool ConnectPipe(HANDLE pipe, HANDLE worker, DWORD timeout_ms) {
  PipeOperation operation;
  if (!operation.overlapped.hEvent) return false;
  const ULONGLONG deadline = GetTickCount64() + timeout_ms;
  const BOOL connected = ConnectNamedPipe(pipe, &operation.overlapped);
  if (!connected && GetLastError() == ERROR_PIPE_CONNECTED) return true;
  DWORD transferred = 0;
  return CompletePipeOperation(pipe, operation, connected, deadline, worker, transferred);
}

bool WriteAll(HANDLE pipe, const std::string &s, DWORD timeout_ms = 15000,
              HANDLE peer = nullptr) {
  PipeOperation operation;
  if (!operation.overlapped.hEvent) return false;
  const ULONGLONG deadline = GetTickCount64() + timeout_ms;
  DWORD written = 0;
  const BOOL immediate = WriteFile(pipe, s.data(), static_cast<DWORD>(s.size()), nullptr,
                                  &operation.overlapped);
  return CompletePipeOperation(pipe, operation, immediate, deadline, peer, written) &&
         written == s.size();
}

bool ReadLine(HANDLE pipe, std::string &line, DWORD timeout_ms, HANDLE peer = nullptr) {
  line.clear();
  const ULONGLONG deadline = GetTickCount64() + timeout_ms;
  char ch = 0;
  while (true) {
    if (GetTickCount64() >= deadline) {
      SetLastError(ERROR_TIMEOUT);
      return false;
    }
    PipeOperation operation;
    if (!operation.overlapped.hEvent) return false;
    DWORD read = 0;
    const BOOL immediate = ReadFile(pipe, &ch, 1, nullptr, &operation.overlapped);
    if (!CompletePipeOperation(pipe, operation, immediate, deadline, peer, read) || read == 0) {
      return false;
    }
    if (ch == '\n') {
      if (!line.empty() && line.back() == '\r') {
        line.pop_back();
      }
      return true;
    }
    line.push_back(ch);
    if (line.size() > 4096) {
      return false;
    }
  }
}

}  // namespace

bool IsProcessElevated() {
  HANDLE token = nullptr;
  if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) {
    return false;
  }
  TOKEN_ELEVATION elev{};
  DWORD size = 0;
  const BOOL ok =
      GetTokenInformation(token, TokenElevation, &elev, sizeof(elev), &size);
  CloseHandle(token);
  return ok && elev.TokenIsElevated != 0;
}

std::wstring CurrentProcessUserSid() {
  HANDLE token = nullptr;
  if (!OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &token)) {
    return {};
  }
  DWORD bytes = 0;
  GetTokenInformation(token, TokenUser, nullptr, 0, &bytes);
  if (bytes == 0 || GetLastError() != ERROR_INSUFFICIENT_BUFFER) {
    CloseHandle(token);
    return {};
  }
  std::vector<std::uint8_t> storage(bytes);
  if (!GetTokenInformation(token, TokenUser, storage.data(), bytes, &bytes)) {
    CloseHandle(token);
    return {};
  }
  CloseHandle(token);

  const auto *token_user = reinterpret_cast<const TOKEN_USER *>(storage.data());
  LPWSTR sid_text = nullptr;
  if (!ConvertSidToStringSidW(token_user->User.Sid, &sid_text) || sid_text == nullptr) {
    return {};
  }
  std::wstring result(sid_text);
  LocalFree(sid_text);
  return result;
}

bool CanWriteDirectory(const std::wstring &directory) {
  std::error_code ec;
  std::filesystem::create_directories(directory, ec);
  if (ec) {
    return false;
  }
  const auto probe = std::filesystem::path(directory) / L".exv_write_probe";
  HANDLE file = CreateFileW(probe.c_str(), GENERIC_WRITE, 0, nullptr, CREATE_ALWAYS,
                            FILE_ATTRIBUTE_TEMPORARY | FILE_FLAG_DELETE_ON_CLOSE, nullptr);
  if (file == INVALID_HANDLE_VALUE) {
    return false;
  }
  CloseHandle(file);
  return true;
}

std::wstring GenerateElevationToken() {
  std::uint8_t bytes[16] = {};
  // Prefer RtlGenRandom via SystemFunction036 if available; fallback to tick+pid.
  HMODULE adv = GetModuleHandleW(L"advapi32.dll");
  using RtlGenRandomFn = BOOLEAN(APIENTRY *)(PVOID, ULONG);
  auto fn = adv ? reinterpret_cast<RtlGenRandomFn>(GetProcAddress(adv, "SystemFunction036"))
                : nullptr;
  if (fn == nullptr || !fn(bytes, sizeof(bytes))) {
    const auto t = GetTickCount64();
    const auto p = GetCurrentProcessId();
    std::memcpy(bytes, &t, sizeof(t) > 8 ? 8 : sizeof(t));
    std::memcpy(bytes + 8, &p, sizeof(p));
  }
  wchar_t hex[33] = {};
  for (int i = 0; i < 16; ++i) {
    std::swprintf(hex + i * 2, 3, L"%02x", bytes[i]);
  }
  return hex;
}

std::wstring DefaultElevationPipeName() {
  return L"\\\\.\\pipe\\exv-setup-elev-" + std::to_wstring(GetCurrentProcessId());
}

bool RunElevatedWorker(const std::wstring &pipe_name, const std::wstring &token,
                       ElevatedServiceOperation operation, std::wstring *error) {
  if (error) {
    *error = L"提权服务操作未完成（启动、认证或通信失败）";
  }
  const auto self = GetSelfPath();
  if (self.empty()) {
    return false;
  }

  // Create pipe server first so worker can connect.
  HANDLE pipe = CreateNamedPipeW(pipe_name.c_str(),
                                 PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                                 PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                                 1,
                                 4096,
                                 4096,
                                 20000,
                                 nullptr);
  if (pipe == INVALID_HANDLE_VALUE) {
    return false;
  }

  const std::wstring params = L"/elevated-worker --pipe \"" + pipe_name + L"\" --token \"" +
                              token + L"\"";

  SHELLEXECUTEINFOW sei{};
  sei.cbSize = sizeof(sei);
  sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
  sei.lpVerb = L"runas";
  sei.lpFile = self.c_str();
  sei.lpParameters = params.c_str();
  sei.nShow = SW_HIDE;

  if (!ShellExecuteExW(&sei)) {
    if (error) {
      *error = L"无法启动提权服务操作（Win32=" + std::to_wstring(GetLastError()) + L"）";
    }
    CloseHandle(pipe);
    return false;
  }

  auto io_failed = [&](const wchar_t *step) {
    if (error) {
      *error = std::wstring(step) + L"（Win32=" + std::to_wstring(GetLastError()) + L"）";
    }
    return false;
  };
  auto exchange = [&]() {
    if (!sei.hProcess) {
      SetLastError(ERROR_INVALID_HANDLE);
      return io_failed(L"未获得提权进程句柄");
    }
    if (!ConnectPipe(pipe, sei.hProcess, 20000)) {
      return io_failed(L"等待提权进程连接失败");
    }
    if (!WriteAll(pipe, "HELLO " + std::string(token.begin(), token.end()) + "\n",
                  15000, sei.hProcess)) {
      return io_failed(L"发送提权认证失败");
    }
    std::string reply;
    if (!ReadLine(pipe, reply, 15000, sei.hProcess)) {
      return io_failed(L"等待提权认证回复失败");
    }
    if (reply != "OK") {
      if (error) *error = L"提权进程认证未通过";
      return false;
    }
    auto call = [&](const char *op, DWORD timeout_ms) {
      // 写入和读取共享同一命令期限，不能每收一个字节重新延长等待。
      const ULONGLONG deadline = GetTickCount64() + timeout_ms;
      if (!WriteAll(pipe, std::string("OP ") + op + "\n", timeout_ms, sei.hProcess)) {
        return io_failed(L"发送提权服务命令失败");
      }
      const ULONGLONG now = GetTickCount64();
      std::string reply;
      if (!ReadLine(pipe, reply, now >= deadline ? 0 : static_cast<DWORD>(deadline - now),
                    sei.hProcess)) {
        return io_failed(L"等待提权服务命令回复失败");
      }
      if (reply != "OK" && error) *error = std::wstring(reply.begin(), reply.end());
      return reply == "OK";
    };
    if (!call(operation == ElevatedServiceOperation::Start ? "start_service" : "stop_service",
               60000)) {
      return false;
    }
    return call("quit", 2000);
  };
  bool ok = exchange();
  // exchange 返回时所有 I/O 均已完成或取消。关管道也会唤醒仍在读命令的 worker。
  DisconnectNamedPipe(pipe);
  CloseHandle(pipe);
  if (sei.hProcess) {
    const DWORD wait = WaitForSingleObject(sei.hProcess, 2000);
    if (wait != WAIT_OBJECT_0) {
      if (ok && error) *error = L"提权服务操作已回复，但进程未按时退出";
      ok = false;
      TerminateProcess(sei.hProcess, 1);
      WaitForSingleObject(sei.hProcess, 2000);
    }
    DWORD code = 1;
    GetExitCodeProcess(sei.hProcess, &code);
    CloseHandle(sei.hProcess);
    return ok && code == 0;
  }
  return ok;
}

bool EnsureServiceStopped() {
  const auto presence = QueryServicePresence(kEngineServiceName);
  if (presence == ServicePresence::NotInstalled) {
    return true;
  }
  if (presence == ServicePresence::Error) {
    return false;
  }
  if (IsProcessElevated()) {
    return StopService(kEngineServiceName);
  }
  return RunElevatedWorker(DefaultElevationPipeName(), GenerateElevationToken());
}

bool EnsureServiceRunning(std::wstring *error) {
  if (IsProcessElevated()) {
    return StartServiceAndWait(kEngineServiceName, error);
  }
  return RunElevatedWorker(DefaultElevationPipeName(), GenerateElevationToken(),
                           ElevatedServiceOperation::Start, error);
}

int RunElevatedWorkerServer(const std::wstring &pipe_name, const std::wstring &token) {
  // Worker is launched elevated; connect as client to parent's pipe.
  HANDLE pipe = INVALID_HANDLE_VALUE;
  for (int i = 0; i < 100; ++i) {
    pipe = CreateFileW(pipe_name.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr,
                       OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
    if (pipe != INVALID_HANDLE_VALUE) {
      break;
    }
    Sleep(50);
  }
  if (pipe == INVALID_HANDLE_VALUE) {
    return 1;
  }

  std::string line;
  if (!ReadLine(pipe, line, 10000)) {
    CloseHandle(pipe);
    return 1;
  }
  const std::string expected = "HELLO " + std::string(token.begin(), token.end());
  if (line != expected) {
    WriteAll(pipe, "ERR bad token\n");
    CloseHandle(pipe);
    return 1;
  }
  WriteAll(pipe, "OK\n");

  while (ReadLine(pipe, line, 120000)) {
    if (line.rfind("OP ", 0) != 0) {
      WriteAll(pipe, "ERR bad op\n");
      continue;
    }
    const auto op = line.substr(3);
    if (op == "quit") {
      WriteAll(pipe, "OK\n");
      break;
    }
    if (op == "stop_service") {
      const bool ok = StopService(kEngineServiceName);
      WriteAll(pipe, ok ? "OK\n" : "ERR service\n");
      continue;
    }
    if (op == "start_service") {
      std::wstring error;
      const bool ok = StartServiceAndWait(kEngineServiceName, &error);
      // 服务控制诊断使用 ASCII 的 API 名称与数值，父进程负责中文业务说明。
      WriteAll(pipe, ok ? "OK\n" : "ERR " + std::string(error.begin(), error.end()) + "\n");
      continue;
    }
    WriteAll(pipe, "ERR unknown\n");
  }
  CloseHandle(pipe);
  return 0;
}

}  // namespace exv::setup
