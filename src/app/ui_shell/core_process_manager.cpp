#include "app/ui_shell/core_process_manager.hpp"

#include <algorithm>
#include <filesystem>
#include <memory>
#include <string>
#include <utility>

#if defined(_WIN32)
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#endif

namespace ecnuvpn::ui_shell {

namespace {

#if defined(_WIN32)

void close_handle(HANDLE &handle) {
  if (handle && handle != INVALID_HANDLE_VALUE) {
    CloseHandle(handle);
    handle = nullptr;
  }
}

std::wstring wide_from_utf8(const std::string &value) {
  if (value.empty()) {
    return {};
  }
  const int size = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                                       value.data(),
                                       static_cast<int>(value.size()), nullptr,
                                       0);
  if (size <= 0) {
    return {};
  }
  std::wstring out(static_cast<std::size_t>(size), L'\0');
  MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.data(),
                      static_cast<int>(value.size()), out.data(), size);
  return out;
}

std::wstring quote_windows_argument(const std::string &value) {
  if (value.empty()) {
    return L"\"\"";
  }

  const bool needs_quotes =
      value.find_first_of(" \t\n\v\"") != std::string::npos;
  std::wstring wide = wide_from_utf8(value);
  if (!needs_quotes) {
    return wide;
  }

  std::wstring quoted;
  quoted.push_back(L'"');
  std::size_t backslashes = 0;
  for (wchar_t ch : wide) {
    if (ch == L'\\') {
      ++backslashes;
      continue;
    }
    if (ch == L'"') {
      quoted.append(backslashes * 2 + 1, L'\\');
      quoted.push_back(ch);
      backslashes = 0;
      continue;
    }
    quoted.append(backslashes, L'\\');
    backslashes = 0;
    quoted.push_back(ch);
  }
  quoted.append(backslashes * 2, L'\\');
  quoted.push_back(L'"');
  return quoted;
}

std::wstring build_windows_command_line(const CoreProcessLaunch &launch) {
  std::wstring command = quote_windows_argument(launch.exv_path);
  for (const std::string &arg : build_core_process_arguments(launch)) {
    command.push_back(L' ');
    command.append(quote_windows_argument(arg));
  }
  return command;
}

class WindowsCoreProcessTransport final : public CoreRpcTransport {
public:
  explicit WindowsCoreProcessTransport(const CoreProcessLaunch &launch) {
    start(launch);
  }

  ~WindowsCoreProcessTransport() override {
    close_handle(stdin_write_);

    if (process_) {
      const DWORD wait_result = WaitForSingleObject(process_, 2000);
      if (wait_result == WAIT_TIMEOUT) {
        TerminateProcess(process_, 1);
        WaitForSingleObject(process_, 1000);
      }
    }

    close_handle(stdout_read_);
    close_handle(process_);
  }

  WindowsCoreProcessTransport(const WindowsCoreProcessTransport &) = delete;
  WindowsCoreProcessTransport &operator=(const WindowsCoreProcessTransport &) =
      delete;

  bool write_line(const std::string &line) override {
    if (!is_started()) {
      return false;
    }

    std::string payload = line;
    payload.push_back('\n');

    const char *cursor = payload.data();
    std::size_t remaining = payload.size();
    while (remaining > 0) {
      const DWORD chunk =
          static_cast<DWORD>(std::min<std::size_t>(remaining, 64 * 1024));
      DWORD written = 0;
      if (!WriteFile(stdin_write_, cursor, chunk, &written, nullptr) ||
          written == 0) {
        return false;
      }
      cursor += written;
      remaining -= written;
    }
    return true;
  }

  bool read_line(std::string &line) override {
    if (!is_started()) {
      return false;
    }
    if (pop_buffered_line(line)) {
      return true;
    }

    while (read_stdout_chunk(0)) {
      if (pop_buffered_line(line)) {
        return true;
      }
    }
    return false;
  }

  bool read_available_line(std::string &line) override {
    if (!is_started()) {
      return false;
    }
    if (pop_buffered_line(line)) {
      return true;
    }

    DWORD available = 0;
    if (!PeekNamedPipe(stdout_read_, nullptr, 0, nullptr, &available, nullptr) ||
        available == 0) {
      return false;
    }

    while (available > 0) {
      if (!read_stdout_chunk(available)) {
        return false;
      }
      if (pop_buffered_line(line)) {
        return true;
      }
      available = 0;
      if (!PeekNamedPipe(stdout_read_, nullptr, 0, nullptr, &available,
                         nullptr)) {
        return false;
      }
    }
    return false;
  }

private:
  void start(const CoreProcessLaunch &launch) {
    if (launch.exv_path.empty() || !launch.use_stdin) {
      return;
    }

    SECURITY_ATTRIBUTES security_attributes{};
    security_attributes.nLength = sizeof(security_attributes);
    security_attributes.bInheritHandle = TRUE;

    HANDLE stdin_read = nullptr;
    HANDLE stdout_write = nullptr;
    if (!CreatePipe(&stdin_read, &stdin_write_, &security_attributes, 0)) {
      return;
    }
    if (!SetHandleInformation(stdin_write_, HANDLE_FLAG_INHERIT, 0)) {
      close_handle(stdin_read);
      close_handle(stdin_write_);
      return;
    }

    if (!CreatePipe(&stdout_read_, &stdout_write, &security_attributes, 0)) {
      close_handle(stdin_read);
      close_handle(stdin_write_);
      return;
    }
    if (!SetHandleInformation(stdout_read_, HANDLE_FLAG_INHERIT, 0)) {
      close_handle(stdin_read);
      close_handle(stdin_write_);
      close_handle(stdout_read_);
      close_handle(stdout_write);
      return;
    }

    STARTUPINFOW startup_info{};
    startup_info.cb = sizeof(startup_info);
    startup_info.dwFlags = STARTF_USESTDHANDLES;
    startup_info.hStdInput = stdin_read;
    startup_info.hStdOutput = stdout_write;
    startup_info.hStdError = GetStdHandle(STD_ERROR_HANDLE);

    PROCESS_INFORMATION process_info{};
    std::wstring app_path = wide_from_utf8(launch.exv_path);
    std::wstring command_line = build_windows_command_line(launch);
    std::wstring current_dir;
    const std::filesystem::path parent =
        std::filesystem::path(launch.exv_path).parent_path();
    if (!parent.empty()) {
      current_dir = parent.wstring();
    }

    const BOOL created = CreateProcessW(
        app_path.empty() ? nullptr : app_path.c_str(), command_line.data(),
        nullptr, nullptr, TRUE, CREATE_NO_WINDOW, nullptr,
        current_dir.empty() ? nullptr : current_dir.c_str(), &startup_info,
        &process_info);

    close_handle(stdin_read);
    close_handle(stdout_write);

    if (!created) {
      close_handle(stdin_write_);
      close_handle(stdout_read_);
      return;
    }

    process_ = process_info.hProcess;
    CloseHandle(process_info.hThread);
  }

  bool is_started() const {
    return process_ && stdin_write_ && stdout_read_;
  }

  bool read_stdout_chunk(DWORD available_bytes) {
    char buffer[4096];
    const DWORD bytes_to_read =
        available_bytes == 0
            ? static_cast<DWORD>(sizeof(buffer))
            : std::min<DWORD>(available_bytes, static_cast<DWORD>(sizeof(buffer)));
    DWORD bytes_read = 0;
    if (!ReadFile(stdout_read_, buffer, bytes_to_read, &bytes_read, nullptr) ||
        bytes_read == 0) {
      return false;
    }
    read_buffer_.append(buffer, buffer + bytes_read);
    return true;
  }

  bool pop_buffered_line(std::string &line) {
    const std::string::size_type newline = read_buffer_.find('\n');
    if (newline == std::string::npos) {
      return false;
    }
    line = read_buffer_.substr(0, newline);
    if (!line.empty() && line.back() == '\r') {
      line.pop_back();
    }
    read_buffer_.erase(0, newline + 1);
    return true;
  }

  HANDLE process_ = nullptr;
  HANDLE stdin_write_ = nullptr;
  HANDLE stdout_read_ = nullptr;
  std::string read_buffer_;
};

#endif

class ClosedCoreProcessTransport final : public CoreRpcTransport {
public:
  bool write_line(const std::string &) override {
    return false;
  }

  bool read_line(std::string &) override {
    return false;
  }
};

} // namespace

std::vector<std::string> build_core_process_arguments(
    const CoreProcessLaunch &launch) {
  std::vector<std::string> args{"--mode=core"};
  if (!launch.state_dir.empty()) {
    args.emplace_back("--config-dir");
    args.emplace_back(launch.state_dir);
  }
  if (!launch.runtime_dir.empty()) {
    args.emplace_back("--home");
    args.emplace_back(launch.runtime_dir);
  }
  if (!launch.use_stdin) {
    args.emplace_back("--daemon");
  }
  return args;
}

std::unique_ptr<CoreRpcTransport> create_core_process_transport(
    const CoreProcessLaunch &launch) {
#if defined(_WIN32)
  return std::make_unique<WindowsCoreProcessTransport>(launch);
#else
  (void)launch;
  return std::make_unique<ClosedCoreProcessTransport>();
#endif
}

} // namespace ecnuvpn::ui_shell
