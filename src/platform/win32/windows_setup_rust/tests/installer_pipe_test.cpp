// 直接编译生产管道实现，SCM 操作仅替换为无副作用桩；测试不改变本机服务。
#include "windows_setup_rust/elevate.cpp"
#include <iostream>

namespace exv::setup {
ServicePresence QueryServicePresence(const std::wstring &) { return ServicePresence::NotInstalled; }
bool StopService(const std::wstring &) { return true; }
bool StartServiceAndWait(const std::wstring &, std::wstring *) { return true; }
}

// 不使用 assert：Release/NDEBUG 下也必须实际执行每次 I/O 并验证结果。
#define CHECK(expression) do { if (!(expression)) { \
  std::cerr << "FAIL line " << __LINE__ << ": " #expression << " error=" << GetLastError() << '\n'; \
  return 1; } } while (false)

int main(int argc, char **) {
  if (argc > 1) return 0;
  using namespace exv::setup;
  const std::wstring name = L"\\\\.\\pipe\\exv-installer-pipe-test-" + std::to_wstring(GetCurrentProcessId());
  HANDLE pipe = CreateNamedPipeW(name.c_str(), PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
      PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, nullptr);
  CHECK(pipe != INVALID_HANDLE_VALUE);
  auto started = GetTickCount64();
  CHECK(!ConnectPipe(pipe, nullptr, 40));
  CHECK(GetLastError() == ERROR_TIMEOUT);
  CHECK(GetTickCount64() - started < 1500);
  HANDLE client = CreateFileW(name.c_str(), GENERIC_READ | GENERIC_WRITE, 0, nullptr,
      OPEN_EXISTING, FILE_FLAG_OVERLAPPED, nullptr);
  CHECK(client != INVALID_HANDLE_VALUE);
  CHECK(ConnectPipe(pipe, nullptr, 100));
  std::string line;
  started = GetTickCount64();
  CHECK(!ReadLine(pipe, line, 40));
  CHECK(GetLastError() == ERROR_TIMEOUT);
  CHECK(GetTickCount64() - started < 1500);
  CHECK(WriteAll(client, "TOKEN fixture-token\n", 100));
  CHECK(ReadLine(pipe, line, 100));
  CHECK(line == "TOKEN fixture-token");
  CHECK(WriteAll(pipe, "OK\n", 100));
  CHECK(ReadLine(client, line, 100));
  CHECK(line == "OK");
  CHECK(WriteAll(client, "partial", 100));
  CloseHandle(client);
  CHECK(!ReadLine(pipe, line, 100));
  CHECK(DisconnectNamedPipe(pipe));
  wchar_t self[MAX_PATH]{};
  CHECK(GetModuleFileNameW(nullptr, self, MAX_PATH));
  std::wstring command = L"\"" + std::wstring(self) + L"\" --exit";
  STARTUPINFOW si{};
  si.cb = sizeof(si);
  PROCESS_INFORMATION pi{};
  CHECK(CreateProcessW(self, command.data(), nullptr, nullptr, FALSE, CREATE_NO_WINDOW,
                       nullptr, nullptr, &si, &pi));
  started = GetTickCount64();
  CHECK(!ConnectPipe(pipe, pi.hProcess, 1000));
  CHECK(GetLastError() == ERROR_PROCESS_ABORTED);
  CHECK(GetTickCount64() - started < 1500);
  CloseHandle(pi.hThread);
  CloseHandle(pi.hProcess);
  CloseHandle(pipe);
  // 所有 I/O 已回收，原名称应可重新创建（取消无悬挂的内核引用）。
  pipe = CreateNamedPipeW(name.c_str(), PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
      PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT, 1, 4096, 4096, 0, nullptr);
  CHECK(pipe != INVALID_HANDLE_VALUE);
  CloseHandle(pipe);
  std::cout << "PASS: connect timeout, silent read timeout, cancellation reuse, token round-trip, "
               "partial EOF rejection, worker exit, resource reclamation\n";
}
