#include "helper.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/process_control.hpp"
#include "platform/common/vpn_supervisor_process.hpp"
#include "vpn.hpp"
#include "vpn_engine/native_session_store.hpp"
#include "vpn_engine/protocol/native_auth_session_json.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <optional>
#include <sstream>
#include <string>
#include <thread>
#include <vector>

#include <windows.h>

namespace {

constexpr const char *kFakeSupervisorModeEnv =
    "ECNUVPN_TEST_FAKE_SUPERVISOR_MODE";
constexpr const char *kFakeSupervisorPidFileEnv =
    "ECNUVPN_TEST_FAKE_SUPERVISOR_PID_FILE";
constexpr const char *kSupervisorPidRegistryEnv =
    "ECNUVPN_SUPERVISOR_PID_REGISTRY";
constexpr const char *kPoisonSupervisorPidEnv =
    "ECNUVPN_TEST_POISON_SUPERVISOR_PID";

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

bool expect_json_ok(const nlohmann::json &value, const char *message) {
  if (value.value("ok", false))
    return true;

  std::cerr << "DETAIL: " << value.dump() << std::endl;
  return expect(false, message);
}

std::string random_endpoint() {
  return "\\\\.\\pipe\\exv-oneshot-test-" +
         std::to_string(GetCurrentProcessId()) + "-" +
         std::to_string(GetTickCount64());
}

std::filesystem::path unique_temp_root(const char *label) {
  return std::filesystem::temp_directory_path() /
         (std::string("ecnuvpn-") + label + "-" +
          std::to_string(GetCurrentProcessId()) + "-" +
          std::to_string(GetTickCount64()));
}

class ScopedEnvVar {
public:
  ScopedEnvVar(const char *name, const std::string &value) : name_(name) {
    const DWORD required = GetEnvironmentVariableA(name_, nullptr, 0);
    if (required > 0) {
      std::string original(static_cast<std::size_t>(required), '\0');
      const DWORD written =
          GetEnvironmentVariableA(name_, original.data(), required);
      if (written > 0 && written < required) {
        original.resize(written);
        original_ = std::move(original);
      }
    }
    SetEnvironmentVariableA(name_, value.c_str());
  }

  ~ScopedEnvVar() {
    if (original_.has_value()) {
      SetEnvironmentVariableA(name_, original_->c_str());
    } else {
      SetEnvironmentVariableA(name_, nullptr);
    }
  }

  ScopedEnvVar(const ScopedEnvVar &) = delete;
  ScopedEnvVar &operator=(const ScopedEnvVar &) = delete;

private:
  const char *name_;
  std::optional<std::string> original_;
};

class ScopedChildProcess {
public:
  ~ScopedChildProcess() {
    if (started_) {
      if (ecnuvpn::platform::is_process_alive(pid()))
        ecnuvpn::platform::terminate_process(pid(), true);
      CloseHandle(process_.hThread);
      CloseHandle(process_.hProcess);
    }
  }

  ScopedChildProcess(const ScopedChildProcess &) = delete;
  ScopedChildProcess &operator=(const ScopedChildProcess &) = delete;
  ScopedChildProcess() = default;

  bool start_sleeping_cmd() {
    std::string command = "cmd.exe /c ping -n 60 127.0.0.1 >nul";
    std::vector<char> mutable_command(command.begin(), command.end());
    mutable_command.push_back('\0');

    STARTUPINFOA startup = {};
    startup.cb = sizeof(startup);
    started_ = CreateProcessA(nullptr, mutable_command.data(), nullptr, nullptr,
                              FALSE, CREATE_NO_WINDOW, nullptr, nullptr,
                              &startup, &process_) != FALSE;
    return started_;
  }

  bool start_current_exe(const std::string &argument) {
    char executable[MAX_PATH] = {};
    if (GetModuleFileNameA(nullptr, executable, MAX_PATH) == 0)
      return false;

    std::string command =
        std::string("\"") + executable + "\" " + argument;
    std::vector<char> mutable_command(command.begin(), command.end());
    mutable_command.push_back('\0');

    STARTUPINFOA startup = {};
    startup.cb = sizeof(startup);
    started_ = CreateProcessA(executable, mutable_command.data(), nullptr,
                              nullptr, FALSE, CREATE_NO_WINDOW, nullptr,
                              nullptr, &startup, &process_) != FALSE;
    return started_;
  }

  int pid() const { return static_cast<int>(process_.dwProcessId); }

private:
  PROCESS_INFORMATION process_ = {};
  bool started_ = false;
};

ecnuvpn::Config sample_native_config() {
  ecnuvpn::Config config;
  config.server = "https://vpn.example.invalid";
  config.username = "student@example.invalid";
  config.mtu = 1280;
  config.useragent = "ECNU-VPN oneshot helper timeout test";
  config.disable_dtls = true;
  config.remember_password = false;
  config.routes = {"10.0.0.0/8", "192.0.2.1"};
  config.log_file = "oneshot-timeout-test.log";
  config.webui_port = 18081;
  config.webui_bind = "127.0.0.1";
  config.webui_enabled = false;
  config.vpn_engine = "native";
  config.openconnect_runtime = "bundled";
  config.windows_tunnel_driver = "wintun";
  config.windows_tap_interface = "ECNU VPN";
  config.auto_reconnect = true;
  config.minimal_mode = true;
  config.service_install_prompt_seen = true;
  config.minimal_install_service_before_connect = false;
  return config;
}

ecnuvpn::vpn_engine::protocol::NativeAuthSession sample_auth_session() {
  ecnuvpn::vpn_engine::protocol::NativeAuthSession session;
  session.server.scheme = "https";
  session.server.host = "vpn.example.invalid";
  session.server.port = 443;
  session.server.base_path = "/";
  session.username = "student@example.invalid";
  session.useragent = "ECNU-VPN oneshot helper timeout auth-session test";
  session.cookie_header = "webvpn=super-cookie-secret";
  session.selected_group = "student";
  session.auth_method = "password";
  session.created_at =
      std::chrono::system_clock::time_point{std::chrono::milliseconds{
          1712345678123LL}};
  return session;
}

bool wait_for_file(const std::filesystem::path &path, int timeout_ms) {
  const ULONGLONG deadline = GetTickCount64() + timeout_ms;
  while (GetTickCount64() < deadline) {
    if (std::filesystem::exists(path))
      return true;
    Sleep(50);
  }
  return std::filesystem::exists(path);
}

int read_pid_file(const std::filesystem::path &path) {
  std::ifstream in(path);
  int pid = -1;
  in >> pid;
  return pid;
}

bool wait_for_process_exit(int pid, int timeout_ms) {
  const ULONGLONG deadline = GetTickCount64() + timeout_ms;
  while (GetTickCount64() < deadline) {
    if (!ecnuvpn::platform::is_process_alive(pid))
      return true;
    Sleep(100);
  }
  return !ecnuvpn::platform::is_process_alive(pid);
}

void wake_acceptor_once(const std::string &endpoint) {
  HANDLE pipe = CreateFileA(endpoint.c_str(), GENERIC_READ | GENERIC_WRITE, 0,
                            NULL, OPEN_EXISTING, 0, NULL);
  if (pipe != INVALID_HANDLE_VALUE) {
    CloseHandle(pipe);
  }
}

bool oneshot_helper_responds_to_hello() {
  const std::string endpoint = random_endpoint();

  ecnuvpn::helper::DaemonOptions options;
  options.mode = "oneshot";
  options.endpoint = endpoint;
  options.auth_token = "oneshot-test-token";
  options.auth_required = true;
  options.oneshot = true;

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = ecnuvpn::helper::daemon_main(options); });

  const ecnuvpn::platform::HelperEndpoint helper_endpoint{
      endpoint, options.auth_token};
  const auto hello = ecnuvpn::platform::send_helper_request(
      helper_endpoint, {{"action", "hello"}});

  bool ok = true;
  ok = expect_json_ok(hello,
                      "one-shot helper should answer hello after startup") &&
       ok;

  ecnuvpn::helper::request_daemon_stop();
  if (hello.value("ok", false)) {
    (void)ecnuvpn::platform::send_helper_request(
        helper_endpoint, {{"action", "hello"}});
  } else {
    wake_acceptor_once(endpoint);
  }

  daemon.join();
  ok = expect(daemon_rc == 0, "one-shot helper should exit with rc=0") && ok;
  return ok;
}

int fake_nonready_supervisor_main() {
  std::ostringstream raw_payload;
  raw_payload << std::cin.rdbuf();

  ecnuvpn::platform::SupervisorStartPayload payload;
  const auto parsed = ecnuvpn::platform::parse_vpn_supervisor_payload(
      raw_payload.str(), &payload);
  if (!parsed.ok)
    return 21;

  ecnuvpn::vpn_engine::NativeSessionRecord record;
  record.pid = -1;
  record.supervisor_pid = static_cast<int>(GetCurrentProcessId());
  record.server = payload.config.server;
  record.route_count = static_cast<int>(payload.config.routes.size());
  record.retry_limit = payload.retry_limit;

  ecnuvpn::vpn_engine::TunnelMetadata metadata;
  metadata.interface_name.clear();
  metadata.internal_ip4_address = "172.20.146.122";
  record.session.tunnel_configured(metadata);
  record.session.packet_loop_started();

  if (!ecnuvpn::vpn_engine::save_native_session_state(payload.config_dir,
                                                      record)) {
    return 22;
  }

  const char *pid_path = std::getenv(kFakeSupervisorPidFileEnv);
  if (pid_path && *pid_path) {
    std::ofstream out(pid_path);
    out << GetCurrentProcessId();
  }

  for (;;) {
    Sleep(1000);
  }
}

int fake_registered_failure_supervisor_main() {
  std::ostringstream raw_payload;
  raw_payload << std::cin.rdbuf();

  ecnuvpn::platform::SupervisorStartPayload payload;
  const auto parsed = ecnuvpn::platform::parse_vpn_supervisor_payload(
      raw_payload.str(), &payload);
  if (!parsed.ok)
    return 31;

  ecnuvpn::vpn_engine::NativeSessionRecord record;
  record.pid = -1;
  record.supervisor_pid = -1;
  record.server = payload.config.server;
  record.route_count = static_cast<int>(payload.config.routes.size());
  record.retry_limit = payload.retry_limit;
  record.session.failed("native_metadata_incomplete",
                        "registered supervisor startup failed");

  const char *poison_pid = std::getenv(kPoisonSupervisorPidEnv);
  if (poison_pid && *poison_pid) {
    try {
      record.supervisor_pid = std::stoi(poison_pid);
    } catch (...) {
      return 33;
    }
  }

  if (!ecnuvpn::vpn_engine::save_native_session_state(payload.config_dir,
                                                      record)) {
    return 32;
  }

  if (poison_pid && *poison_pid) {
    std::ofstream out(std::filesystem::path(payload.config_dir) /
                      "ecnuvpn-supervisor.pid");
    out << poison_pid;
  }

  const char *pid_path = std::getenv(kFakeSupervisorPidFileEnv);
  if (pid_path && *pid_path) {
    std::ofstream out(pid_path);
    out << GetCurrentProcessId();
  }

  for (;;) {
    Sleep(1000);
  }
}

int fake_idle_supervisor_main() {
  const char *pid_path = std::getenv(kFakeSupervisorPidFileEnv);
  if (pid_path && *pid_path) {
    std::ofstream out(pid_path);
    out << GetCurrentProcessId();
  }

  for (;;) {
    Sleep(1000);
  }
}

bool oneshot_helper_start_failure_returns_specific_code_and_stops_supervisor() {
  namespace fs = std::filesystem;

  const std::string endpoint = random_endpoint();
  const fs::path root = unique_temp_root("oneshot-timeout");
  const fs::path home_dir = root / "home";
  const fs::path config_dir = root / "config";
  const fs::path pid_file = root / "fake-supervisor.pid";
  fs::create_directories(home_dir);
  fs::create_directories(config_dir);

  ScopedEnvVar fake_mode(kFakeSupervisorModeEnv, "nonready_loop");
  ScopedEnvVar fake_pid_file(kFakeSupervisorPidFileEnv, pid_file.string());

  ecnuvpn::helper::DaemonOptions options;
  options.mode = "oneshot";
  options.endpoint = endpoint;
  options.auth_token = "oneshot-timeout-token";
  options.auth_required = true;
  options.oneshot = true;

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = ecnuvpn::helper::daemon_main(options); });

  const ecnuvpn::platform::HelperEndpoint helper_endpoint{
      endpoint, options.auth_token};
  const auto hello = ecnuvpn::platform::send_helper_request(
      helper_endpoint, {{"action", "hello"}});

  bool ok = true;
  ok = expect_json_ok(hello, "one-shot helper should answer hello before start") &&
       ok;

  nlohmann::json request{
      {"action", "start"},
      {"config", sample_native_config()},
      {"native_start_mode", "auth_session"},
      {"retry_limit", -1},
      {"home", home_dir.string()},
      {"config_dir", config_dir.string()},
      {"auth_session",
       ecnuvpn::vpn_engine::protocol::to_json(sample_auth_session())}};

  const auto response =
      ecnuvpn::platform::send_helper_request(helper_endpoint, request);

  ok = expect(response.is_object(), "start response should be a JSON object") &&
       ok;
  ok = expect(!response.value("ok", true),
              "timed-out startup should be reported as a helper error") &&
       ok;
  ok = expect(response.value("code", std::string()) ==
                  "native_metadata_incomplete",
              "metadata-incomplete startup should surface native_metadata_incomplete") &&
       ok;
  ok = expect(
           response.value("message", std::string()).find(
               "did not report a usable tunnel interface") != std::string::npos,
           "metadata-incomplete startup should use the stable readiness failure message") &&
       ok;

  ok = expect(wait_for_file(pid_file, 2000),
              "fake supervisor should record its pid before helper returns") &&
       ok;
  const int supervisor_pid = read_pid_file(pid_file);
  ok = expect(supervisor_pid > 0, "fake supervisor pid should be readable") &&
       ok;
  if (supervisor_pid > 0) {
    ok = expect(wait_for_process_exit(supervisor_pid, 5000),
                "one-shot helper should stop the supervisor after startup failure") &&
         ok;
  }

  ecnuvpn::helper::request_daemon_stop();
  wake_acceptor_once(endpoint);
  daemon.join();
  ok = expect(daemon_rc == 0,
              "one-shot helper should exit cleanly after failed start cleanup") &&
       ok;

  std::error_code cleanup_ec;
  fs::remove_all(root, cleanup_ec);
  return ok;
}

bool oneshot_helper_failed_native_start_uses_registered_supervisor_cleanup_and_exits() {
  namespace fs = std::filesystem;

  const std::string endpoint = random_endpoint();
  const fs::path root = unique_temp_root("oneshot-registered-cleanup");
  const fs::path home_dir = root / "home";
  const fs::path config_dir = root / "config";
  const fs::path registry_file = root / "supervisor-registry.txt";
  const fs::path real_supervisor_pid_file = root / "real-supervisor.pid";
  fs::create_directories(home_dir);
  fs::create_directories(config_dir);

  ScopedEnvVar fake_mode(kFakeSupervisorModeEnv, "registered_failure_loop");
  ScopedEnvVar fake_pid_file(kFakeSupervisorPidFileEnv,
                             real_supervisor_pid_file.string());
  ScopedEnvVar supervisor_registry(kSupervisorPidRegistryEnv,
                                   registry_file.string());
  ScopedChildProcess stale_process;
  bool ok = true;
  ok = expect(stale_process.start_current_exe("__non-supervisor-loop"),
              "test should create a same-exe non-supervisor process for stale pid poisoning") &&
       ok;
  ScopedEnvVar poison_supervisor_pid(kPoisonSupervisorPidEnv,
                                     std::to_string(stale_process.pid()));

  ecnuvpn::helper::DaemonOptions options;
  options.mode = "oneshot";
  options.endpoint = endpoint;
  options.auth_token = "oneshot-registered-cleanup-token";
  options.auth_required = true;
  options.oneshot = true;

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = ecnuvpn::helper::daemon_main(options); });

  const ecnuvpn::platform::HelperEndpoint helper_endpoint{
      endpoint, options.auth_token};
  const auto hello = ecnuvpn::platform::send_helper_request(
      helper_endpoint, {{"action", "hello"}});

  ok = expect_json_ok(hello, "one-shot helper should answer hello before start") &&
       ok;

  nlohmann::json request{
      {"action", "start"},
      {"config", sample_native_config()},
      {"native_start_mode", "auth_session"},
      {"retry_limit", -1},
      {"home", home_dir.string()},
      {"config_dir", config_dir.string()},
      {"auth_session",
       ecnuvpn::vpn_engine::protocol::to_json(sample_auth_session())}};

  const auto response =
      ecnuvpn::platform::send_helper_request(helper_endpoint, request);

  ok = expect(response.is_object(), "start response should be a JSON object") &&
       ok;
  ok = expect(!response.value("ok", true),
              "registered supervisor startup failure should be reported") &&
       ok;
  ok = expect(response.value("code", std::string()) ==
                  "native_metadata_incomplete",
              "registered supervisor failure should preserve the native failure code") &&
       ok;

  ok = expect(wait_for_file(real_supervisor_pid_file, 2000),
              "fake supervisor should record its pid before helper returns") &&
       ok;
  const int supervisor_pid = read_pid_file(real_supervisor_pid_file);
  ok = expect(supervisor_pid > 0,
              "fake supervisor pid should be readable") &&
       ok;

  daemon.join();
  ok = expect(daemon_rc == 0,
              "one-shot helper should exit after terminal failed start without an explicit stop") &&
       ok;

  if (supervisor_pid > 0) {
    const bool supervisor_exited = wait_for_process_exit(supervisor_pid, 5000);
    ok = expect(supervisor_exited,
                "failed start cleanup should kill registered supervisor pids") &&
         ok;
    if (!supervisor_exited) {
      ecnuvpn::platform::terminate_process(supervisor_pid, true);
      (void)wait_for_process_exit(supervisor_pid, 5000);
    }
  }
  ok = expect(ecnuvpn::platform::is_process_alive(stale_process.pid()),
              "failed start cleanup must not terminate a stale same-exe non-supervisor pid") &&
       ok;

  std::error_code cleanup_ec;
  fs::remove_all(root, cleanup_ec);
  return ok;
}

bool oneshot_helper_stop_without_session_scans_request_config_dir() {
  namespace fs = std::filesystem;

  const std::string endpoint = random_endpoint();
  const fs::path root = unique_temp_root("oneshot-stop-custom-config");
  const fs::path config_dir = root / "custom-config";
  const fs::path supervisor_pid_file = root / "idle-supervisor.pid";
  fs::create_directories(config_dir);

  ScopedEnvVar fake_mode(kFakeSupervisorModeEnv, "idle_loop");
  ScopedEnvVar fake_pid_file(kFakeSupervisorPidFileEnv,
                             supervisor_pid_file.string());
  ScopedChildProcess supervisor;

  bool ok = true;
  ok = expect(supervisor.start_current_exe("__vpn-supervisor"),
              "test should create a same-exe supervisor process") &&
       ok;
  ok = expect(wait_for_file(supervisor_pid_file, 2000),
              "idle supervisor should record its pid") &&
       ok;

  ecnuvpn::vpn_engine::NativeSessionRecord record;
  record.supervisor_pid = supervisor.pid();
  record.server = "vpn.example.invalid";
  record.retry_limit = -1;
  record.session.auth_started();
  ok = expect(ecnuvpn::vpn_engine::save_native_session_state(config_dir.string(),
                                                            record),
              "test should save native state in custom config dir") &&
       ok;

  ecnuvpn::helper::DaemonOptions options;
  options.mode = "oneshot";
  options.endpoint = endpoint;
  options.auth_token = "oneshot-stop-custom-config-token";
  options.auth_required = true;
  options.oneshot = true;

  int daemon_rc = -1;
  std::thread daemon([&]() { daemon_rc = ecnuvpn::helper::daemon_main(options); });

  const ecnuvpn::platform::HelperEndpoint helper_endpoint{
      endpoint, options.auth_token};
  const auto hello = ecnuvpn::platform::send_helper_request(
      helper_endpoint, {{"action", "hello"}});
  ok = expect_json_ok(hello, "one-shot helper should answer hello before stop") &&
       ok;

  const auto response = ecnuvpn::platform::send_helper_request(
      helper_endpoint,
      {{"action", "stop"}, {"config_dir", config_dir.string()}});
  ok = expect(response.is_object(), "stop response should be a JSON object") &&
       ok;
  ok = expect(!response.value("ok", true),
              "stop without helper session state should still report not running") &&
       ok;

  daemon.join();
  ok = expect(daemon_rc == 0,
              "one-shot helper should exit after terminal stop request") &&
       ok;

  const bool supervisor_exited = wait_for_process_exit(supervisor.pid(), 5000);
  ok = expect(supervisor_exited,
              "stop without session state should cleanup request custom config supervisor") &&
       ok;

  std::error_code cleanup_ec;
  fs::remove_all(root, cleanup_ec);
  return ok;
}

} // namespace

int main(int argc, char *argv[]) {
  if (argc > 1 && std::string(argv[1]) == "__non-supervisor-loop") {
    for (;;) {
      Sleep(1000);
    }
  }

  if (argc > 2 && std::string(argv[1]) == "__helper-exec")
    return ecnuvpn::helper::worker_main(argv[2]);

  if (argc > 1 && std::string(argv[1]) == "__vpn-supervisor") {
    const char *mode = std::getenv(kFakeSupervisorModeEnv);
    if (mode && std::string(mode) == "idle_loop")
      return fake_idle_supervisor_main();
    if (mode && std::string(mode) == "nonready_loop")
      return fake_nonready_supervisor_main();
    if (mode && std::string(mode) == "registered_failure_loop")
      return fake_registered_failure_supervisor_main();
    return ecnuvpn::vpn::supervisor_main();
  }

  bool ok = true;
  ok = oneshot_helper_responds_to_hello() && ok;
  ok = oneshot_helper_start_failure_returns_specific_code_and_stops_supervisor() &&
       ok;
  ok =
      oneshot_helper_failed_native_start_uses_registered_supervisor_cleanup_and_exits() &&
      ok;
  ok = oneshot_helper_stop_without_session_scans_request_config_dir() && ok;
  return ok ? 0 : 1;
}
