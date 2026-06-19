#include "core/pipe_ipc.hpp"
#include "platform/common/core_resolver_platform_deps.hpp"

#include <chrono>
#include <filesystem>
#include <fstream>
#include <future>
#include <iostream>
#include <sstream>
#include <string>
#include <thread>

#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <unistd.h>
#endif

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << "\n";
  return false;
}

std::string unique_pipe_path() {
#ifdef _WIN32
  return "\\\\.\\pipe\\exv-pipe-ipc-test-" +
         std::to_string(static_cast<unsigned long>(GetCurrentProcessId())) +
         "-" +
         std::to_string(std::chrono::steady_clock::now()
                            .time_since_epoch()
                            .count());
#else
  return "/tmp/exv-pipe-ipc-test-" + std::to_string(getpid()) + "-" +
         std::to_string(std::chrono::steady_clock::now()
                            .time_since_epoch()
                            .count()) +
         ".sock";
#endif
}

#ifdef _WIN32
std::string send_windows_pipe_request(const std::string &pipe_path) {
  HANDLE pipe = INVALID_HANDLE_VALUE;
  for (int attempt = 0; attempt < 20; ++attempt) {
    pipe = CreateFileA(pipe_path.c_str(), GENERIC_READ | GENERIC_WRITE, 0,
                       nullptr, OPEN_EXISTING, 0, nullptr);
    if (pipe != INVALID_HANDLE_VALUE) {
      break;
    }
    std::this_thread::sleep_for(std::chrono::milliseconds(10));
  }
  if (pipe == INVALID_HANDLE_VALUE) {
    return "connect_failed";
  }

  const std::string request = "{\"id\":7,\"action\":\"logs.list\"}\n";
  DWORD written = 0;
  if (!WriteFile(pipe, request.data(), static_cast<DWORD>(request.size()),
                 &written, nullptr)) {
    CloseHandle(pipe);
    return "write_failed";
  }

  char buffer[256] = {};
  DWORD read = 0;
  if (!ReadFile(pipe, buffer, sizeof(buffer) - 1, &read, nullptr) ||
      read == 0) {
    CloseHandle(pipe);
    return "read_failed";
  }
  CloseHandle(pipe);
  return std::string(buffer, read);
}
#endif

bool probe_connection_does_not_poison_next_request() {
  const std::string pipe_path = unique_pipe_path();
  exv::core::PipeIpcListener listener(pipe_path);
  if (!expect(listener.start(), "listener should start")) {
    return false;
  }

#ifdef _WIN32
  auto deps = exv::core::lifecycle::make_platform_core_resolver_deps();
  const bool probe_connected = deps.try_connect_ipc(pipe_path);
  const bool probe_observed =
      listener.accept_one([](const std::string &) { return "{}"; });

  auto response_future = std::async(std::launch::async, [&] {
    return send_windows_pipe_request(pipe_path);
  });

  bool handled = false;
  const auto deadline = std::chrono::steady_clock::now() +
                        std::chrono::seconds(2);
  while (std::chrono::steady_clock::now() < deadline && !handled) {
    handled = listener.accept_one([](const std::string &request) {
      return request.find("logs.list") != std::string::npos
                 ? "{\"id\":7,\"ok\":true,\"data\":[]}"
                 : "{\"id\":7,\"ok\":false,\"message\":\"wrong request\"}";
    });
    if (!handled) {
      std::this_thread::sleep_for(std::chrono::milliseconds(10));
    }
  }

  const auto response = response_future.get();
  listener.stop();

  bool ok = true;
  ok = expect(probe_connected, "probe client should connect") && ok;
  ok = expect(!probe_observed,
              "resolver probe should not create an empty pipe request") &&
       ok;
  ok = expect(handled,
              "listener should process request after a probe disconnect") &&
       ok;
  ok = expect(response.find("\"ok\":true") != std::string::npos,
              "client after probe should receive response") &&
       ok;
  return ok;
#else
  listener.stop();
  return true;
#endif
}

bool windows_core_launch_does_not_inherit_parent_handles() {
#ifdef _WIN32
  const auto source_root = std::filesystem::path(__FILE__).parent_path().parent_path();
  const auto source_path =
      source_root / "src" / "platform" / "common" /
      "core_resolver_platform_deps.cpp";
  std::ifstream in(source_path);
  if (!expect(in.good(), "should open core resolver platform deps source")) {
    return false;
  }
  std::ostringstream buffer;
  buffer << in.rdbuf();
  const std::string source = buffer.str();

  bool ok = true;
  ok = expect(source.find("WaitNamedPipeA(ipc_path.c_str(), 50) != 0") !=
                  std::string::npos,
              "Windows resolver probe should only succeed when a pipe instance is available") &&
       ok;
  ok = expect(source.find("GetLastError() == ERROR_SEM_TIMEOUT") ==
                  std::string::npos,
              "Windows resolver probe must not treat WaitNamedPipe timeout as connected") &&
       ok;
  ok = expect(source.find("CreateProcessA(nullptr, cmd_buf, nullptr, nullptr, FALSE") !=
                  std::string::npos,
              "Windows core launch must not inherit CLI/UI pipe handles") &&
       ok;
  ok = expect(source.find("CREATE_NO_WINDOW | DETACHED_PROCESS") !=
                  std::string::npos,
              "Windows core launch should remain detached") &&
       ok;
  return ok;
#else
  return true;
#endif
}

} // namespace

int main() {
  bool ok = true;
  ok = probe_connection_does_not_poison_next_request() && ok;
  ok = windows_core_launch_does_not_inherit_parent_handles() && ok;
  return ok ? 0 : 1;
}
