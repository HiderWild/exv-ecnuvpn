#include "windows_setup_rust/elevate.hpp"

#include "windows_setup_rust/pfro_sweep.hpp"
#include "windows_setup_rust/service_control.hpp"

#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shellapi.h>
#include <sddl.h>

#include <cstdio>
#include <cstring>
#include <filesystem>
#include <mutex>
#include <string>
#include <cstdint>
#include <thread>
#include <vector>

namespace exv::setup {
namespace {

// S2（2026-10-10 单次 UAC 提权会话）会话态预算：worker 命令间空闲上限 600s
// （worker 侧 ReadLine 预算，原单命令 120s），客户端心跳间隔 30s——必须覆盖
// 最坏安装时长（慢盘解压+写入全链，停服与起服之间的全部非提权步骤），心跳
// 让 worker 永不触顶；单条服务命令期限维持 60s。
constexpr DWORD kSessionIdleLimitMs = 600000;
constexpr DWORD kHeartbeatIntervalMs = 30000;
constexpr DWORD kHeartbeatOpTimeoutMs = 15000;
constexpr DWORD kServiceOpTimeoutMs = 60000;


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

// S5（2026-10-10 换盘服务迁移）：migrate_service 的新 ImagePath 经窄字符行
// 协议传递，必须走 UTF-8 无损编解码——std::string(w.begin(), w.end()) 式窄化
// 直转会破坏非 ASCII 路径（既有 ERR 回复仅 ASCII 诊断，不受影响）。转换失败
// （无效代理项等）返回空串，调用侧 fail-closed。
std::string WideToUtf8(const std::wstring &text) {
  if (text.empty()) {
    return {};
  }
  const int size = WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()),
                                       nullptr, 0, nullptr, nullptr);
  if (size <= 0) {
    return {};
  }
  std::string out(static_cast<std::size_t>(size), '\0');
  WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), out.data(), size,
                      nullptr, nullptr);
  return out;
}

std::wstring Utf8ToWide(const std::string &text) {
  if (text.empty()) {
    return {};
  }
  const int size = MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()),
                                       nullptr, 0);
  if (size <= 0) {
    return {};
  }
  std::wstring out(static_cast<std::size_t>(size), L'\0');
  MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), out.data(), size);
  return out;
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

// S2 注入缝（复审 MINOR-3）：runas 启动的唯一入口。生产实现真弹 UAC
// （ShellExecuteExW "runas"）；installer_pipe_test 直接编译本文件并把
// g_elevated_launch 替换为无 UAC 的假 worker 子进程——会话的 launch_count
// （= 经此缝的启动次数）即 UAC 计数。真机单 UAC 人工验收归 W5。
namespace {

struct ElevatedLaunchSpec {
  std::wstring exe_path;
  std::wstring pipe_name;
  std::wstring token;
  std::wstring pfro_sweep_dir;  // 空=纯服务会话，不携带该旗标
};
using ElevatedLaunchFn = bool (*)(const ElevatedLaunchSpec &spec, HANDLE *worker_process);

bool LaunchElevatedViaRunas(const ElevatedLaunchSpec &spec, HANDLE *worker_process) {
  // 旗标组合与 cli.cpp 提权 worker 白名单一致（S1 定案，勿动）。
  std::wstring params = L"/elevated-worker --pipe \"" + spec.pipe_name + L"\" --token \"" +
                        spec.token + L"\"";
  if (!spec.pfro_sweep_dir.empty()) {
    params += L" --pfro-sweep-dir \"" + spec.pfro_sweep_dir + L"\"";
  }
  SHELLEXECUTEINFOW sei{};
  sei.cbSize = sizeof(sei);
  sei.fMask = SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
  sei.lpVerb = L"runas";
  sei.lpFile = spec.exe_path.c_str();
  sei.lpParameters = params.c_str();
  sei.nShow = SW_HIDE;
  if (!ShellExecuteExW(&sei)) {
    return false;
  }
  *worker_process = sei.hProcess;
  return true;
}

ElevatedLaunchFn g_elevated_launch = &LaunchElevatedViaRunas;

// S2 客户端会话对象（生产实现）：一次 runas 启动 worker 后，会话内多条命令
// 复用同一进程；空闲期 30s 心跳 ping 维持 worker 命令循环；会话结束 quit。
// 传输级失败（worker 死亡/超时）回收后按需重建（=再弹一次 UAC）并重试一次，
// launch_count_ 如实累计。
class RunasElevatedSession : public ElevatedSession {
 public:
  explicit RunasElevatedSession(const std::wstring &pfro_sweep_dir)
      : pfro_sweep_dir_(pfro_sweep_dir) {}
  ~RunasElevatedSession() override { Shutdown(); }
  RunasElevatedSession(const RunasElevatedSession &) = delete;
  RunasElevatedSession &operator=(const RunasElevatedSession &) = delete;

  bool Call(const char *op, std::wstring *error) override {
    return CallLine(std::string("OP ") + op, error);
  }

  bool Call(const char *op, const std::wstring &arg, std::wstring *error) override {
    // S5：带参命令——参数经 UTF-8 无损编码后取「OP <op> <arg> 行尾全段」形态
    //（worker 侧不作空白切分，路径可含空格）。编码失败或含行协议控制字符时
    // fail-closed 拒发，不把不完整命令交给 worker。
    const std::string encoded = WideToUtf8(arg);
    if ((encoded.empty() && !arg.empty()) || encoded.find('\n') != std::string::npos ||
        encoded.find('\r') != std::string::npos || encoded.find('\0') != std::string::npos) {
      if (error) {
        *error = L"提权服务命令参数编码失败（路径含非法字符）";
      }
      return false;
    }
    return CallLine(std::string("OP ") + op + " " + encoded, error);
  }

  void Shutdown() override {
    std::thread heartbeat;
    {
      std::lock_guard<std::mutex> lock(mutex_);
      if (alive_ && pipe_ != INVALID_HANDLE_VALUE) {
        // 会话结束 quit：worker 回复后自退；限时等待+回收在 Reap 完成。
        WriteAll(pipe_, "OP quit\n", 2000, worker_);
        std::string reply;
        ReadLine(pipe_, reply, 2000, worker_);
      }
      alive_ = false;
      heartbeat = std::move(heartbeat_thread_);
    }
    // join 必须在锁外：心跳线程 ping 要先拿锁，锁内 join 必死锁。
    if (heartbeat_wake_ != nullptr) {
      SetEvent(heartbeat_wake_);
    }
    if (heartbeat.joinable()) {
      heartbeat.join();
    }
    std::lock_guard<std::mutex> lock(mutex_);
    if (heartbeat_wake_ != nullptr) {
      CloseHandle(heartbeat_wake_);
      heartbeat_wake_ = nullptr;
    }
    ReapWorkerLocked();
  }

  int launch_count() const override { return launch_count_; }

 private:
  enum class ExchangeOutcome { Ok, Rejected, Transport };

  // 无参/带参 Call 的公共交换循环：命令行已在调用侧拼好（"OP <name>[ <arg>]"）。
  bool CallLine(const std::string &op_line, std::wstring *error) {
    std::lock_guard<std::mutex> lock(mutex_);
    if (error) {
      *error = L"提权服务操作未完成（启动、认证或通信失败）";
    }
    for (int attempt = 0; attempt < 2; ++attempt) {
      if (!EnsureWorkerLocked(error)) {
        return false;
      }
      switch (ExchangeLocked(op_line, error)) {
        case ExchangeOutcome::Ok:
          return true;
        case ExchangeOutcome::Rejected:
          return false;  // 命令级失败：worker 仍存活，如实上抛不重试。
        case ExchangeOutcome::Transport:
          // 传输级失败（worker 中途死亡/超时）：回收后按需重建重试一次
          //（重建=再弹一次 UAC，launch_count_ 已如实累计）。
          DropWorkerLocked();
          break;
      }
    }
    return false;
  }

  bool EnsureWorkerLocked(std::wstring *error) {
    if (alive_) {
      // 廉价死亡探测：worker 已退出（空闲超时自退/崩溃）时回收重建，不把死
      // 会话的传输失败强加给下一条命令。
      if (WaitForSingleObject(worker_, 0) != WAIT_OBJECT_0) {
        return true;
      }
      ReapWorkerLocked();
    } else if (worker_ != nullptr || pipe_ != INVALID_HANDLE_VALUE) {
      // 心跳判死后线程不自取句柄：重建前先回收残留。
      DropWorkerLocked();
    }
    return LaunchWorkerLocked(error);
  }

  bool LaunchWorkerLocked(std::wstring *error) {
    const auto self = GetSelfPath();
    if (self.empty()) {
      return false;
    }
    // 计数先于启动尝试：UAC 弹出即计（拒绝/失败也如实计入）。
    ++launch_count_;
    const std::wstring pipe_name =
        DefaultElevationPipeName() + L"-s" + std::to_wstring(launch_count_);
    token_ = GenerateElevationToken();

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

    HANDLE worker = nullptr;
    const bool launched =
        g_elevated_launch(ElevatedLaunchSpec{self, pipe_name, token_, pfro_sweep_dir_}, &worker);
    if (!launched) {
      if (error) {
        *error = L"无法启动提权服务操作（Win32=" + std::to_wstring(GetLastError()) + L"）";
      }
      CloseHandle(pipe);
      return false;
    }
    auto abandon = [&]() {
      DisconnectNamedPipe(pipe);
      CloseHandle(pipe);
      if (worker != nullptr) {
        const DWORD wait = WaitForSingleObject(worker, 2000);
        if (wait != WAIT_OBJECT_0) {
          TerminateProcess(worker, 1);
          WaitForSingleObject(worker, 2000);
        }
        CloseHandle(worker);
      }
    };
    auto fail_launch = [&](const wchar_t *step) {
      if (error) {
        *error = std::wstring(step) + L"（Win32=" + std::to_wstring(GetLastError()) + L"）";
      }
      abandon();
      return false;
    };
    if (worker == nullptr) {
      SetLastError(ERROR_INVALID_HANDLE);
      return fail_launch(L"未获得提权进程句柄");
    }
    if (!ConnectPipe(pipe, worker, 20000)) {
      return fail_launch(L"等待提权进程连接失败");
    }
    if (!WriteAll(pipe, "HELLO " + std::string(token_.begin(), token_.end()) + "\n",
                  15000, worker)) {
      return fail_launch(L"发送提权认证失败");
    }
    std::string reply;
    if (!ReadLine(pipe, reply, 15000, worker)) {
      return fail_launch(L"等待提权认证回复失败");
    }
    if (reply != "OK") {
      if (error) {
        *error = L"提权进程认证未通过";
      }
      abandon();
      return false;
    }
    pipe_ = pipe;
    worker_ = worker;
    alive_ = true;
    StartHeartbeatLocked();
    return true;
  }

  ExchangeOutcome ExchangeLocked(const std::string &op_line, std::wstring *error) {
    // 写入和读取共享同一命令期限，不能每收一个字节重新延长等待。
    const ULONGLONG deadline = GetTickCount64() + kServiceOpTimeoutMs;
    if (!WriteAll(pipe_, op_line + "\n", kServiceOpTimeoutMs, worker_)) {
      if (error) {
        *error = L"发送提权服务命令失败（Win32=" + std::to_wstring(GetLastError()) + L"）";
      }
      return ExchangeOutcome::Transport;
    }
    const ULONGLONG now = GetTickCount64();
    std::string reply;
    const DWORD remaining = now >= deadline ? 0 : static_cast<DWORD>(deadline - now);
    if (!ReadLine(pipe_, reply, remaining, worker_)) {
      if (error) {
        *error = L"等待提权服务命令回复失败（Win32=" + std::to_wstring(GetLastError()) + L"）";
      }
      return ExchangeOutcome::Transport;
    }
    if (reply != "OK") {
      if (error) {
        *error = std::wstring(reply.begin(), reply.end());
      }
      return ExchangeOutcome::Rejected;
    }
    return ExchangeOutcome::Ok;
  }

  void DropWorkerLocked() {
    // 传输级失败后 worker 可能仍挂起（未退出）：强制终止，不留提权进程。
    if (worker_ != nullptr) {
      if (WaitForSingleObject(worker_, 0) != WAIT_OBJECT_0) {
        TerminateProcess(worker_, 1);
        WaitForSingleObject(worker_, 2000);
      }
      CloseHandle(worker_);
      worker_ = nullptr;
    }
    if (pipe_ != INVALID_HANDLE_VALUE) {
      DisconnectNamedPipe(pipe_);
      CloseHandle(pipe_);
      pipe_ = INVALID_HANDLE_VALUE;
    }
    alive_ = false;
  }

  void ReapWorkerLocked() {
    if (pipe_ != INVALID_HANDLE_VALUE) {
      // 关管道也会唤醒仍在读命令的 worker。
      DisconnectNamedPipe(pipe_);
      CloseHandle(pipe_);
      pipe_ = INVALID_HANDLE_VALUE;
    }
    if (worker_ != nullptr) {
      const DWORD wait = WaitForSingleObject(worker_, 2000);
      if (wait != WAIT_OBJECT_0) {
        TerminateProcess(worker_, 1);
        WaitForSingleObject(worker_, 2000);
      }
      CloseHandle(worker_);
      worker_ = nullptr;
    }
    alive_ = false;
  }

  void StartHeartbeatLocked() {
    if (heartbeat_wake_ == nullptr) {
      heartbeat_wake_ = CreateEventW(nullptr, FALSE, FALSE, nullptr);
    }
    if (heartbeat_wake_ == nullptr || heartbeat_thread_.joinable()) {
      return;
    }
    try {
      heartbeat_thread_ = std::thread([this] { HeartbeatLoop(); });
    } catch (...) {
      // 心跳线程创建失败：会话仍可用，仅失去保活（worker 600s 空闲自退后
      // 由下一提权步骤按需重建）。
    }
  }

  void HeartbeatLoop() {
    while (true) {
      const DWORD wait = heartbeat_wake_ != nullptr
                             ? WaitForSingleObject(heartbeat_wake_, kHeartbeatIntervalMs)
                             : (Sleep(kHeartbeatIntervalMs), WAIT_TIMEOUT);
      if (wait != WAIT_TIMEOUT) {
        break;
      }
      std::lock_guard<std::mutex> lock(mutex_);
      if (!alive_) {
        continue;
      }
      // 空闲期 ping：与前台命令共用互斥串行，30s 间隔远小于 worker 600s
      // 空闲上限，覆盖慢盘解压+写入全链的最坏安装时长。
      std::string reply;
      if (!WriteAll(pipe_, "OP ping\n", kHeartbeatOpTimeoutMs, worker_) ||
          !ReadLine(pipe_, reply, kHeartbeatOpTimeoutMs, worker_) || reply != "OK") {
        // ping 失败=会话死亡：线程不自取句柄（避免与前台/析构竞争），回收
        // 交给下一次 Call 的重建或 Shutdown。
        alive_ = false;
      }
    }
  }

  const std::wstring pfro_sweep_dir_;
  std::wstring token_;
  HANDLE pipe_ = INVALID_HANDLE_VALUE;
  HANDLE worker_ = nullptr;
  bool alive_ = false;
  int launch_count_ = 0;
  std::mutex mutex_;
  HANDLE heartbeat_wake_ = nullptr;
  std::thread heartbeat_thread_;
};

}  // namespace

std::unique_ptr<ElevatedSession> CreateElevationSession(const std::wstring &pfro_sweep_dir) {
  return std::make_unique<RunasElevatedSession>(pfro_sweep_dir);
}

bool EnsureServiceStopped(ElevatedSession &session) {
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
  return session.Call("stop_service");
}

bool EnsureServiceRunning(ElevatedSession &session, std::wstring *error) {
  if (IsProcessElevated()) {
    return StartServiceAndWait(kEngineServiceName, error);
  }
  return session.Call("start_service", error);
}

bool EnsureServiceImagePathMigrated(ElevatedSession &session,
                                    const std::wstring &new_engine_path, std::wstring *error) {
  if (IsProcessElevated()) {
    return MigrateServiceImagePath(new_engine_path, kEngineServiceName, error);
  }
  if (error) {
    *error = L"提权服务迁移未完成（启动、认证或通信失败）";
  }
  return session.Call("migrate_service", new_engine_path, error);
}

int RunElevatedWorkerServer(const std::wstring &pipe_name, const std::wstring &token,
                            const std::wstring &pfro_sweep_dir) {
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

  // S2：命令间空闲上限 600s（会话态）；客户端 30s 心跳 ping 维持，超时自退
  // （会话陈旧时 worker 不留驻）。
  while (ReadLine(pipe, line, kSessionIdleLimitMs)) {
    if (line.rfind("OP ", 0) != 0) {
      WriteAll(pipe, "ERR bad op\n");
      continue;
    }
    const auto op = line.substr(3);
    if (op == "quit") {
      WriteAll(pipe, "OK\n");
      break;
    }
    if (op == "ping") {
      // S2 会话心跳：空闲期客户端 30s 一次，防误触上面的空闲上限。
      WriteAll(pipe, "OK\n");
      continue;
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

    if (op.rfind("migrate_service ", 0) == 0) {
      // S5（2026-10-10 换盘服务迁移）：新 ImagePath 为行尾全段（UTF-8 无损，
      // 路径可含空格/非 ASCII，不作空白切分）。ChangeServiceConfigW 需
      // SERVICE_CHANGE_CONFIG——worker 已提权故可改；尾参保真在
      // MigrateServiceImagePath 内完成。错误仅回 ASCII 诊断，新旧路径的中文
      // 业务文案由父进程（调用侧）拼接。
      const std::wstring new_path = Utf8ToWide(op.substr(sizeof("migrate_service ") - 1));
      if (new_path.empty()) {
        WriteAll(pipe, "ERR migrate path missing\n");
        continue;
      }
      std::wstring error;
      const bool ok = MigrateServiceImagePath(new_path, kEngineServiceName, &error);
      WriteAll(pipe, ok ? "OK\n" : "ERR " + std::string(error.begin(), error.end()) + "\n");
      continue;
    }

    if (op == "sweep_pfro") {
      // 2026-10-10：安装 GUI（asInvoker）撤销 EXV 重启删除登记的提权路径。
      // install_dir 由 worker 自身命令行（宽字符）携带，不经窄字符管道转码。
      // S2：与服务操作共用同一会话 worker（原独立 -pfro 管道实现已移除）。
      if (pfro_sweep_dir.empty()) {
        WriteAll(pipe, "ERR sweep dir missing\n");
        continue;
      }
      const auto sweep = SweepExvPendingFileRenameOperations(pfro_sweep_dir);
      if (sweep.ok) {
        WriteAll(pipe, "OK\n");
      } else {
        WriteAll(pipe, "ERR " + std::string(sweep.error.begin(), sweep.error.end()) + "\n");
      }
      continue;
    }
    WriteAll(pipe, "ERR unknown\n");
  }
  CloseHandle(pipe);
  return 0;
}

bool RunElevatedPfroSweep(ElevatedSession &session, std::wstring *error) {
  // S2：PFRO 撤销并入同一提权会话——sweep_pfro OP 已在协议内，install_dir 经
  // 会话 worker 命令行宽字符携带。fail-closed 判定留在 install_engine 调用侧
  // （撤销不了就拒装），语义与原独立实现一致。
  if (error) {
    *error = L"提权清理待重启删除项未完成（启动、认证或通信失败）";
  }
  return session.Call("sweep_pfro", error);
}

}  // namespace exv::setup
