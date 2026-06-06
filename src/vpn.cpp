#include "vpn.hpp"
#include "app_api.hpp"
#include "config.hpp"
#include "core/tunnel_controller_active.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "runtime/runtime_context.hpp"
#include "platform/common/helper_lifecycle.hpp"
#include "platform/common/openconnect_process.hpp"
#include "platform/common/process_control.hpp"
#include "platform/common/vpn_supervisor_process.hpp"
#include "openconnect_log.hpp"
#include "tunnel.hpp"
#include "utils.hpp"
#include "virtual_network.hpp"
#include "vpn_engine/event_sink.hpp"
#include "vpn_engine/native_engine.hpp"
#include "vpn_engine/native_session_store.hpp"

#include <atomic>
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstdint>
#include <filesystem>
#include <functional>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <mutex>
#include <sstream>
#include <thread>
#include <utility>
#include <vector>
#ifndef _WIN32
#include <unistd.h>
#include <sys/wait.h>
#else
#include <windows.h>
#ifdef _MSC_VER
typedef int pid_t;
#endif
#endif

namespace ecnuvpn {
namespace vpn {

static volatile sig_atomic_t supervisor_stop_requested = 0;
static volatile sig_atomic_t supervisor_child_pid = -1;

class ConnectTiming {
public:
  explicit ConnectTiming(std::string scope)
      : scope_(std::move(scope)), started_(Clock::now()), last_(started_) {
    logger::info("[connect-timing] scope=" + scope_ +
                 " stage=begin delta_ms=0 total_ms=0");
  }

  void mark(const std::string &stage, const std::string &detail = "") {
    auto now = Clock::now();
    auto delta_ms = elapsed_ms(last_, now);
    auto total_ms = elapsed_ms(started_, now);
    last_ = now;

    std::string message = "[connect-timing] scope=" + scope_ +
                          " stage=" + stage +
                          " delta_ms=" + std::to_string(delta_ms) +
                          " total_ms=" + std::to_string(total_ms);
    if (!detail.empty())
      message += " " + detail;
    logger::info(message);
  }

  void finish(bool ok, const std::string &detail = "") {
    if (finished_)
      return;
    finished_ = true;
    mark(ok ? "finish.ok" : "finish.error", detail);
  }

private:
  using Clock = std::chrono::steady_clock;

  static long long elapsed_ms(const Clock::time_point &from,
                              const Clock::time_point &to) {
    return std::chrono::duration_cast<std::chrono::milliseconds>(to - from)
        .count();
  }

  std::string scope_;
  Clock::time_point started_;
  Clock::time_point last_;
  bool finished_ = false;
};

class RuntimeLogTail {
public:
  using ChunkHandler = std::function<void(const std::string &)>;

  RuntimeLogTail(std::string log_path, ChunkHandler on_chunk)
      : log_path_(std::move(log_path)), on_chunk_(std::move(on_chunk)),
        offset_(current_size()) {
    running_ = true;
    worker_ = std::thread([this]() { run(); });
  }

  ~RuntimeLogTail() { stop(); }

  RuntimeLogTail(const RuntimeLogTail &) = delete;
  RuntimeLogTail &operator=(const RuntimeLogTail &) = delete;

  void stop() {
    bool expected = true;
    if (!running_.compare_exchange_strong(expected, false))
      return;
    if (worker_.joinable())
      worker_.join();
  }

private:
  std::uintmax_t current_size() const {
    std::error_code ec;
    std::uintmax_t size = std::filesystem::file_size(log_path_, ec);
    return ec ? 0 : size;
  }

  void run() {
    while (running_) {
      read_available();
      std::this_thread::sleep_for(std::chrono::milliseconds(50));
    }
    read_available();
  }

  void read_available() {
    std::error_code ec;
    std::uintmax_t size = std::filesystem::file_size(log_path_, ec);
    if (ec)
      return;
    if (size < offset_)
      offset_ = 0;
    if (size == offset_)
      return;

    std::ifstream in(log_path_, std::ios::binary);
    if (!in.is_open())
      return;
    in.seekg(static_cast<std::streamoff>(offset_));
    std::ostringstream buffer;
    buffer << in.rdbuf();
    std::string chunk = buffer.str();
    offset_ += static_cast<std::uintmax_t>(chunk.size());
    if (!chunk.empty() && on_chunk_)
      on_chunk_(chunk);
  }

  std::string log_path_;
  ChunkHandler on_chunk_;
  std::uintmax_t offset_ = 0;
  std::atomic<bool> running_{false};
  std::thread worker_;
};

class AuthFailureWatch {
public:
  explicit AuthFailureWatch(const Config &cfg)
      : tail_(utils::expand_home(cfg.log_file),
              [this](const std::string &chunk) { observe(chunk); }) {}

  bool failed() const { return failed_.load(); }

  void stop() { tail_.stop(); }

private:
  void observe(const std::string &chunk) {
    std::lock_guard<std::mutex> lock(buffer_mutex_);
    buffer_ += chunk;
    if (buffer_.size() > 8192)
      buffer_.erase(0, buffer_.size() - 8192);
    if (openconnect_log::contains_auth_failure_text(buffer_))
      failed_ = true;
  }

  std::atomic<bool> failed_{false};
  std::mutex buffer_mutex_;
  std::string buffer_;
  RuntimeLogTail tail_;
};

class ConnectionDiagnostics {
public:
  explicit ConnectionDiagnostics(std::string scope) : scope_(std::move(scope)) {}

  void event(const std::string &name, const std::string &detail = "") {
    std::string item = name;
    if (!detail.empty())
      item += "(" + detail + ")";
    events_.push_back(item);
    if (events_.size() > 24)
      events_.erase(events_.begin());
  }

  void mark_route_ready(const std::string &interface_name,
                        const std::string &internal_ip) {
    route_ready_ = true;
    event("route_ready", interface_name + "," + internal_ip);
  }

  void mark_process_exited_before_ready() {
    process_exited_before_ready_ = true;
    event("openconnect_exited_before_route_ready");
  }

  bool initial_connection_failed() const {
    return process_exited_before_ready_ && !route_ready_;
  }

  void flush_summary(bool ok) const {
    std::string summary = "[connect-diagnostics] scope=" + scope_ +
                          " result=" + (ok ? "ok" : "failed") +
                          " route_ready=" + (route_ready_ ? "true" : "false") +
                          " process_exited_before_ready=" +
                          (process_exited_before_ready_ ? "true" : "false") +
                          " events=";
    for (std::size_t i = 0; i < events_.size(); ++i) {
      if (i)
        summary += ">";
      summary += events_[i];
    }
    if (ok)
      logger::info(summary);
    else
      logger::warn(summary);
  }

private:
  std::string scope_;
  std::vector<std::string> events_;
  bool route_ready_ = false;
  bool process_exited_before_ready_ = false;
};

static void write_pid_file(const std::string &path, pid_t pid) {
  std::ofstream ofs(path);
  if (ofs.is_open()) {
    ofs << pid;
    ofs.flush();
    utils::sync_owner(path);
  }
}

static pid_t read_pid_file(const std::string &path) {
  if (!utils::file_exists(path))
    return -1;
  std::string content = utils::read_file(path);
  content = utils::trim(content);
  if (content.empty())
    return -1;
  try {
    return static_cast<pid_t>(std::stoi(content));
  } catch (...) {
    return -1;
  }
}

static void remove_pid_file(const std::string &path) {
  if (utils::file_exists(path)) {
    std::remove(path.c_str());
  }
}

static void write_pid(pid_t pid) { write_pid_file(utils::get_pid_path(), pid); }

static pid_t read_pid() { return read_pid_file(utils::get_pid_path()); }

static void remove_pid() { remove_pid_file(utils::get_pid_path()); }

static void write_supervisor_pid(pid_t pid) {
  write_pid_file(utils::get_supervisor_pid_path(), pid);
}

static pid_t read_supervisor_pid() {
  return read_pid_file(utils::get_supervisor_pid_path());
}

static void remove_supervisor_pid() {
  remove_pid_file(utils::get_supervisor_pid_path());
}

static void remove_route_ready() {
  remove_pid_file(utils::get_route_ready_path());
}

static pid_t current_process_pid() {
#ifdef _WIN32
  return static_cast<pid_t>(GetCurrentProcessId());
#else
  return static_cast<pid_t>(getpid());
#endif
}

static bool read_route_ready(std::string *interface_name = nullptr,
                             std::string *internal_ip = nullptr) {
  std::string path = utils::get_route_ready_path();
  if (!utils::file_exists(path))
    return false;

  std::istringstream iss(utils::read_file(path));
  std::string tun;
  std::string ip;
  if (!std::getline(iss, tun) || !std::getline(iss, ip))
    return false;

  tun = utils::trim(tun);
  ip = utils::trim(ip);
  if (tun.empty() || ip.empty())
    return false;

  if (interface_name)
    *interface_name = tun;
  if (internal_ip)
    *internal_ip = ip;
  return true;
}

static void clear_runtime_state() {
  remove_pid();
  remove_supervisor_pid();
  remove_route_ready();
}

static bool is_process_alive(pid_t pid) {
  return platform::is_process_alive(static_cast<int>(pid));
}

static pid_t find_openconnect_pid() {
  return static_cast<pid_t>(platform::find_openconnect_pid());
}

static void terminate_process(pid_t pid, bool force) {
  platform::terminate_process(static_cast<int>(pid), force);
}

static void sleep_ms(unsigned int milliseconds) {
  platform::sleep_ms(milliseconds);
}

static void append_openconnect_attempt_marker(const Config &cfg) {
  std::filesystem::path log_path = utils::expand_home(cfg.log_file);
  std::error_code ec;
  if (log_path.has_parent_path())
    std::filesystem::create_directories(log_path.parent_path(), ec);
  std::ofstream out(log_path, std::ios::app);
  if (out.is_open()) {
    out << "Starting VPN: " << cfg.server << " user=" << cfg.username
        << std::endl;
  }
}

static void handle_supervisor_signal(int) {
  supervisor_stop_requested = 1;
  pid_t child_pid = supervisor_child_pid;
  if (child_pid > 0) {
    terminate_process(child_pid, false);
  }
}

static std::string describe_retry_policy(int retry_limit) {
  return retry_limit < 0 ? std::string("infinite")
                         : std::to_string(retry_limit) + " retries";
}

class LoggingEventSink final : public vpn_engine::EventSink {
public:
  explicit LoggingEventSink(vpn_engine::EventSink *inner) : inner_(inner) {}

  void emit(const vpn_engine::VpnEngineEvent &event) override {
    std::string message = "[native-engine] " + event.type;
    if (!event.message.empty())
      message += ": " + event.message;

    const auto code = event.fields.find("code");
    if (code != event.fields.end() && !code->second.empty())
      message += " (code=" + code->second + ")";

    if (event.level == "error")
      logger::error(message);
    else if (event.level == "warn")
      logger::warn(message);
    else
      logger::info(message);

    if (inner_)
      inner_->emit(event);
  }

private:
  vpn_engine::EventSink *inner_;
};

// LEGACY: Durable owner for native-engine sessions. Only reached when
// TunnelController is NOT available (helper worker, CLI fallback).
// Native engine uses TunnelController (Core-owned mode) via app_api.
//
// The native packet loop runs on a background thread that lives inside the
// *process* hosting the NativeVpnEngineSession, so the session must be owned
// by this long-lived supervisor (not the short-lived CLI/RPC invocation that
// requested the connection). This process records itself as the durable
// supervisor PID in native-session-state.json and holds the session open
// until a stop is requested or the retry budget is exhausted.
static int run_native_supervisor(const Config &cfg, const std::string &password,
                                 int retry_limit) {
  signal(SIGTERM, handle_supervisor_signal);
  signal(SIGINT, handle_supervisor_signal);

  logger::info("Native VPN supervisor started, retry policy: " +
               describe_retry_policy(retry_limit));

  const std::string config_dir = utils::get_config_dir();
  const pid_t own_pid = current_process_pid();
  int reconnect_attempts_used = 0;
  bool first_attempt = true;

  while (!supervisor_stop_requested) {
    if (!first_attempt) {
      if (retry_limit == 0)
        break;
      if (retry_limit > -1 && reconnect_attempts_used >= retry_limit) {
        logger::warn("Native VPN supervisor reached retry limit: " +
                     std::to_string(retry_limit));
        break;
      }
      ++reconnect_attempts_used;
      logger::warn("Native VPN session ended; reconnect attempt " +
                   std::to_string(reconnect_attempts_used) +
                   (retry_limit > -1 ? ("/" + std::to_string(retry_limit))
                                     : " (infinite mode)"));
      sleep_ms(2000);
      if (supervisor_stop_requested)
        break;
    }

    vpn_engine::NativeSessionRecord record;
    record.pid = -1;
    record.supervisor_pid = static_cast<int>(own_pid);
    record.server = cfg.server;
    record.route_count = static_cast<int>(cfg.routes.size());
    record.retry_limit = retry_limit;

    vpn_engine::NativeSessionEventRecorder recorder(config_dir, record);
  LoggingEventSink logging_sink(&recorder);
    vpn_engine::NativeVpnEngineDependencies dependencies =
        vpn_engine::default_native_engine_dependencies();
  dependencies.event_sink = &logging_sink;

    vpn_engine::NativeVpnEngineSession session(
        vpn_engine::make_native_config(cfg, password), dependencies);

    auto result = session.start();
    if (!result.ok) {
      logger::error("Native VPN engine start failed: " + result.code);
      recorder.mark_stopped();
      first_attempt = false;
      continue;
    }

    logger::info("Native VPN session established under durable supervisor, PID: " +
                 std::to_string(own_pid));

    // Own the session for the lifetime of this process: hold it open until the
    // session stops on its own (transport failure) or a stop is requested.
    while (!supervisor_stop_requested) {
      vpn_engine::VpnEngineStatus status = session.status();
      if (!status.running)
        break;
      sleep_ms(250);
    }

    session.stop();
    recorder.mark_stopped();

    if (supervisor_stop_requested)
      break;

    first_attempt = false;
  }

  clear_runtime_state();
  // Do NOT clear native session state here. When the engine fails to reach a
  // network-ready state, the recorder has just persisted the real failure_code
  // to native-session-state.json. handle_start() reads that code after the
  // worker process exits and is responsible for clearing the file afterwards.
  // Clearing here destroys the failure code before the desktop can see it,
  // producing a generic "Failed to establish the VPN connection" error.
  logger::info("Native VPN supervisor stopped");
  return 0;
}

// LEGACY: Reconnect supervisor for legacy_openconnect engine and native-engine
// fallback path. Native engine uses TunnelController (Core-owned mode) via
// app_api when available; this function is only reached when TunnelController
// is not initialized (helper worker, CLI fallback).
static int run_supervisor(const Config &cfg, const std::string &password,
                          int retry_limit) {
  if (cfg.vpn_engine == "native") {
    return run_native_supervisor(cfg, password, retry_limit);
  }

  signal(SIGTERM, handle_supervisor_signal);
  signal(SIGINT, handle_supervisor_signal);

  logger::info("Reconnect supervisor started, retry policy: " +
               describe_retry_policy(retry_limit));

  bool first_attempt = true;
  int reconnect_attempts_used = 0;
  bool retry_limit_reached = false;

  while (!supervisor_stop_requested) {
    ConnectTiming timing(first_attempt ? "vpn.supervisor.initial"
                                       : "vpn.supervisor.reconnect");
    bool timing_finished = false;
    if (!first_attempt) {
      if (retry_limit == 0) {
        timing.finish(false, "reason=reconnect_disabled");
        break;
      }
      if (retry_limit > -1 && reconnect_attempts_used >= retry_limit) {
        retry_limit_reached = true;
        timing.finish(false, "reason=retry_limit_reached");
        break;
      }

      ++reconnect_attempts_used;
      logger::warn("VPN disconnected, attempting reconnect " +
                   std::to_string(reconnect_attempts_used) +
                   (retry_limit > -1 ? ("/" + std::to_string(retry_limit))
                                     : " (infinite mode)"));
#ifndef _WIN32
      sleep(2);
#else
      Sleep(2000);
#endif
      if (supervisor_stop_requested)
        break;
      timing.mark("reconnect_delay_complete",
                  "attempt=" + std::to_string(reconnect_attempts_used));
    }

    pid_t child_pid = -1;
    platform::OpenconnectProcess child_process;
    append_openconnect_attempt_marker(cfg);
    AuthFailureWatch auth_watch(cfg);
    if (!platform::spawn_openconnect_process(cfg, password, &child_process)) {
      timing.finish(false, "reason=spawn_openconnect_failed");
      timing_finished = true;
      if (first_attempt) {
        clear_runtime_state();
        return 1;
      }
      continue;
    }
    child_pid = static_cast<pid_t>(child_process.pid);
    timing.mark("spawn_openconnect", "pid=" + std::to_string(child_pid));

    supervisor_child_pid = child_pid;
    write_pid(child_pid);
    timing.mark("write_pid", "pid=" + std::to_string(child_pid));
    logger::info(std::string(first_attempt ? "Starting" : "Reconnect attempt") +
                 " openconnect, PID: " + std::to_string(child_pid));

    int wait_status = 0;
    bool route_ready_logged = false;
    bool auth_failed_logged = false;
#ifndef _WIN32
    while (true) {
      pid_t wait_result = waitpid(child_pid, &wait_status, WNOHANG);
      if (wait_result == child_pid)
        break;
      if (wait_result < 0) {
        if (errno == EINTR)
          continue;
        logger::error("waitpid failed while supervising openconnect");
        break;
      }

      if (!route_ready_logged) {
        pid_t vpn_pid = read_pid();
        std::string vpn_interface;
        std::string internal_ip;
        if (vpn_pid > 0 && is_process_alive(vpn_pid) &&
            read_route_ready(&vpn_interface, &internal_ip)) {
          logger::info(std::string(first_attempt
                                       ? "VPN connection ready under reconnect supervisor"
                                       : "VPN reconnect succeeded") +
                       ", PID: " + std::to_string(vpn_pid) +
                       ", interface: " + vpn_interface +
                       ", internal IP: " + internal_ip);
          timing.mark("route_ready",
                      "pid=" + std::to_string(vpn_pid) + " interface=" +
                          vpn_interface + " internal_ip=" + internal_ip);
          timing.finish(true, "pid=" + std::to_string(vpn_pid));
          timing_finished = true;
          route_ready_logged = true;
        }
      }

      if (!route_ready_logged && !auth_failed_logged &&
          (auth_watch.failed() || tunnel::runtime_log_has_auth_failure(cfg))) {
        auth_failed_logged = true;
        logger::error("OpenConnect reported authentication failure before network readiness");
        timing.mark("auth_failed");
        terminate_process(child_pid, true);
      }

      usleep(250000);
    }
#else
    DWORD child_exit_code = 1;
    HANDLE hChild = static_cast<HANDLE>(child_process.wait_handle);
    while (true) {
      if (!hChild) {
        logger::error("Invalid Windows openconnect process handle while supervising.");
        break;
      }
      DWORD wait_result = WaitForSingleObject(hChild, 250);
      if (wait_result == WAIT_OBJECT_0) {
        break;
      }
      if (wait_result == WAIT_FAILED) {
        logger::error("WaitForSingleObject failed while supervising openconnect");
        break;
      }

      if (!route_ready_logged) {
        pid_t vpn_pid = read_pid();
        std::string vpn_interface;
        std::string internal_ip;
        if (vpn_pid > 0 && is_process_alive(vpn_pid) &&
            read_route_ready(&vpn_interface, &internal_ip)) {
          logger::info(std::string(first_attempt
                                       ? "VPN connection ready under reconnect supervisor"
                                       : "VPN reconnect succeeded") +
                       ", PID: " + std::to_string(vpn_pid) +
                       ", interface: " + vpn_interface +
                       ", internal IP: " + internal_ip);
          timing.mark("route_ready",
                      "pid=" + std::to_string(vpn_pid) + " interface=" +
                          vpn_interface + " internal_ip=" + internal_ip);
          timing.finish(true, "pid=" + std::to_string(vpn_pid));
          timing_finished = true;
          route_ready_logged = true;
        }
      }

      if (!route_ready_logged && !auth_failed_logged &&
          (auth_watch.failed() || tunnel::runtime_log_has_auth_failure(cfg))) {
        auth_failed_logged = true;
        logger::error("OpenConnect reported authentication failure before network readiness");
        timing.mark("auth_failed");
        terminate_process(child_pid, true);
      }
    }
    if (hChild) {
      GetExitCodeProcess(hChild, &child_exit_code);
    }
    platform::close_openconnect_process(&child_process);
#endif

    if (!route_ready_logged) {
      std::string vpn_interface;
      std::string internal_ip;
      pid_t vpn_pid = read_pid();
      if (vpn_pid > 0 && is_process_alive(vpn_pid) &&
          read_route_ready(&vpn_interface, &internal_ip)) {
        logger::info(std::string(first_attempt
                                     ? "VPN connection ready under reconnect supervisor"
                                     : "VPN reconnect succeeded") +
                     ", PID: " + std::to_string(vpn_pid) +
                     ", interface: " + vpn_interface +
                     ", internal IP: " + internal_ip);
        timing.mark("route_ready",
                    "pid=" + std::to_string(vpn_pid) + " interface=" +
                        vpn_interface + " internal_ip=" + internal_ip);
        timing.finish(true, "pid=" + std::to_string(vpn_pid));
        timing_finished = true;
      }
    }

    supervisor_child_pid = -1;
    remove_pid();
    remove_route_ready();

    if (auth_failed_logged) {
      clear_runtime_state();
      timing.finish(false, "reason=auth_failed");
      return first_attempt ? kVpnInitialConnectFailedExitCode : 1;
    }

    if (supervisor_stop_requested)
      break;

#ifndef _WIN32
    if (WIFEXITED(wait_status)) {
      logger::warn("openconnect exited with code: " +
                   std::to_string(WEXITSTATUS(wait_status)));
    } else if (WIFSIGNALED(wait_status)) {
      logger::warn("openconnect terminated by signal: " +
                   std::to_string(WTERMSIG(wait_status)));
    }
#else
    logger::warn("openconnect exited with code: " +
                 std::to_string(child_exit_code));
#endif
    if (!timing_finished) {
      timing.finish(false, "reason=openconnect_exited_before_route_ready");
    }

    first_attempt = false;
  }

  if (retry_limit_reached) {
    logger::warn("Reconnect supervisor stopped after reaching retry limit: " +
                 std::to_string(retry_limit));
  } else if (supervisor_stop_requested) {
    logger::info("Reconnect supervisor stop requested");
  }

  clear_runtime_state();
  return 0;
}

#ifdef _WIN32
int supervisor_main() {
  try {
    std::ostringstream payload;
    payload << std::cin.rdbuf();
    nlohmann::json request = nlohmann::json::parse(payload.str());
    std::string home = request.value("home", std::string());
    std::string config_dir = request.value("config_dir", std::string());
    runtime::bootstrap(config_dir, home, /*force=*/true);
    logger::init();
    write_supervisor_pid(static_cast<pid_t>(GetCurrentProcessId()));
    Config cfg = request.at("config").get<Config>();
    std::string password = request.at("password").get<std::string>();
    int retry_limit = request.value("retry_limit", 0);
    return run_supervisor(cfg, password, retry_limit);
  } catch (...) {
    clear_runtime_state();
    return 1;
  }
}
#endif

int start(const Config &cfg, int retry_limit) {
  utils::print_header("EXV Starting");

  // ── Pre-flight checks ──────────────────────────────────────
  if (cfg.vpn_engine == "native") {
    auto validation = vpn_engine::validate_native_config(cfg);
    if (!validation.ok) {
      utils::print_error(validation.message);
      logger::error("Native VPN engine config invalid: " + validation.code);
      return 1;
    }
  } else if (!utils::check_openconnect(cfg.openconnect_runtime)) {
#ifdef _WIN32
    if (cfg.openconnect_runtime != "system") {
      utils::print_error("Bundled OpenConnect runtime is missing from this installation.");
      utils::print_info("Rebuild the desktop package with the bundled native runtime assets.");
      logger::error("Bundled OpenConnect runtime missing on Windows");
      return 1;
    }
#elif defined(__APPLE__)
    if (cfg.openconnect_runtime != "system") {
      utils::print_error("Bundled OpenConnect runtime is missing from this installation.");
      utils::print_info("Rebuild the desktop package with the macOS bundled runtime assets.");
      logger::error("Bundled OpenConnect runtime missing on macOS");
      return 1;
    }
#endif
    utils::print_warning("openconnect is not installed.");
    std::cout << std::endl;
    std::cout << utils::BOLD << "  Install openconnect now? [Y/n] "
              << utils::RESET;
    std::string answer;
    std::getline(std::cin, answer);
    // Default to yes on empty input
    if (!answer.empty() && answer[0] != 'y' && answer[0] != 'Y') {
      utils::print_info("Aborting. Install openconnect manually and retry.");
      return 1;
    }

#ifdef __APPLE__
    // Check if Homebrew is available
    if (utils::run_command("which brew > /dev/null 2>&1") != 0) {
      utils::print_error("Homebrew is not installed either.");
      std::cout << std::endl;
      std::cout << utils::BOLD
                << "  Please run the following commands to install:"
                << utils::RESET << std::endl;
      std::cout << std::endl;
      std::cout << utils::YELLOW << "  # 1. Install Homebrew:" << utils::RESET
                << std::endl
                << "  /bin/bash -c \"$(curl -fsSL "
                   "https://raw.githubusercontent.com/Homebrew/install/HEAD/"
                   "install.sh)\""
                << std::endl
                << std::endl;
      std::cout << utils::YELLOW
                << "  # 2. Install openconnect:" << utils::RESET << std::endl
                << "  brew install openconnect" << std::endl
                << std::endl;
      logger::error("openconnect and Homebrew both not found");
      return 1;
    }

    // Homebrew found — attempt to install openconnect
    utils::print_info("Running: brew install openconnect ...");
    std::cout << std::endl;
    int ret = utils::run_command("brew install openconnect");
    std::cout << std::endl;
    if (ret != 0 || !utils::check_openconnect(cfg.openconnect_runtime)) {
      utils::print_error("brew install openconnect failed.");
      utils::print_info("Try running it manually: brew install openconnect");
      logger::error("brew install openconnect failed");
      return 1;
    }
    utils::print_success("openconnect installed successfully!");
    logger::info("openconnect installed via Homebrew");
#elif defined(_WIN32)
    std::cout << "Download openconnect from: https://github.com/openconnect/openconnect-gui/releases\n";
    logger::error("openconnect not found");
    return 1;
#else
    // Linux: try system package managers
    if (utils::run_command("which apt-get > /dev/null 2>&1") == 0) {
      utils::print_info("Running: sudo apt-get install -y openconnect ...");
      int ret = utils::run_command("sudo apt-get install -y openconnect");
      if (ret != 0 || !utils::check_openconnect(cfg.openconnect_runtime)) {
        utils::print_error("apt-get install openconnect failed.");
        return 1;
      }
    } else if (utils::run_command("which dnf > /dev/null 2>&1") == 0) {
      utils::print_info("Running: sudo dnf install -y openconnect ...");
      int ret = utils::run_command("sudo dnf install -y openconnect");
      if (ret != 0 || !utils::check_openconnect(cfg.openconnect_runtime)) {
        utils::print_error("dnf install openconnect failed.");
        return 1;
      }
    } else if (utils::run_command("which pacman > /dev/null 2>&1") == 0) {
      utils::print_info("Running: sudo pacman -S --noconfirm openconnect ...");
      int ret = utils::run_command("sudo pacman -S --noconfirm openconnect");
      if (ret != 0 || !utils::check_openconnect(cfg.openconnect_runtime)) {
        utils::print_error("pacman install openconnect failed.");
        return 1;
      }
    } else {
      utils::print_error("No supported package manager found (apt-get/dnf/pacman).");
      utils::print_info("Install openconnect manually, then run exv again.");
      logger::error("openconnect not found, no package manager available");
      return 1;
    }
    utils::print_success("openconnect installed successfully!");
    logger::info("openconnect installed via system package manager");
#endif
  }

  // Prefer helper daemon even when running as root — it manages session state
  // and the reconnect supervisor, and ensures stop works correctly.
  if (helper::is_available()) {
    Config validated_cfg = cfg;
    if (validated_cfg.username.empty()) {
      utils::print_error("Username not configured!");
      utils::print_info("Use 'exv config set username <your_username>' to set it.");
      return 1;
    }
    if (validated_cfg.remember_password && validated_cfg.password.empty()) {
      utils::print_error("Password not configured!");
      utils::print_info("Run: exv config set password");
      return 1;
    }
    if (validated_cfg.server.empty()) {
      utils::print_error("Server not configured!");
      return 1;
    }

    std::string plaintext_password = config::get_plaintext_password(validated_cfg);
    if (plaintext_password.empty()) {
      return 1;
    }

    return helper::start_via_helper(validated_cfg, plaintext_password, retry_limit)
               ? 0
               : 1;
  }

  if (!utils::check_root()) {
#ifdef _WIN32
    utils::print_error("Administrator privileges required to start the VPN. Install the helper with 'exv service install' from an elevated prompt or run this command as Administrator.");
    logger::error("Not running as Administrator for VPN start and helper is unavailable");
#else
    utils::print_error("Root privileges required to start the VPN. Install the helper with 'sudo exv service install' or run with sudo.");
    logger::error("Not running as root for VPN start and helper is unavailable");
#endif
    return 1;
  }

  // Check if VPN is already running
  pid_t supervisor_pid = read_supervisor_pid();
  if (supervisor_pid > 0 && !is_process_alive(supervisor_pid)) {
    clear_runtime_state();
    supervisor_pid = -1;
  }

  pid_t existing_pid = read_pid();
  if (existing_pid <= 0 || !is_process_alive(existing_pid)) {
    existing_pid = find_openconnect_pid();
  }

  if (supervisor_pid > 0 || existing_pid > 0) {
    utils::print_warning(
        "VPN is already running (PID: " +
        std::to_string(existing_pid > 0 ? existing_pid : supervisor_pid) + ")");
    utils::print_info("Use 'exv stop' to stop the current connection first.");
    return 1;
  }

  // Validate config
  if (cfg.username.empty()) {
    utils::print_error("Username not configured!");
    utils::print_info("Use 'exv config set username <your_username>' to set it.");
    return 1;
  }
  // Only check for stored ciphertext when remember_password is true.
  // When false, get_plaintext_password() will prompt interactively.
  if (cfg.remember_password && cfg.password.empty()) {
    utils::print_error("Password not configured!");
    utils::print_info("Run: exv config set password");
    return 1;
  }
  if (cfg.server.empty()) {
    utils::print_error("Server not configured!");
    return 1;
  }

  // ── Decrypt / prompt password ───────────────────────────────
  std::string plaintext_password = config::get_plaintext_password(cfg);
  if (plaintext_password.empty()) {
    return 1; // error already printed by get_plaintext_password
  }

  return start_with_password(cfg, plaintext_password, retry_limit);
}

int start_with_password(const Config &cfg, const std::string &plaintext_password,
                        int retry_limit) {
  ConnectTiming timing(retry_limit == 0 ? "vpn.start.direct"
                                        : "vpn.start.supervised");
  if (cfg.vpn_engine == "native") {
    // ── D1: Native engine supervisor bypass ─────────────────────────────
    //
    // Post-C1 architecture: the primary native-engine connect flow uses
    // TunnelController (Core-owned mode) which manages the
    // NativeVpnEngineSession via CoreSessionRunner in the core process.
    // The app_api path (vpn.connect) uses TunnelController directly and
    // does NOT call this function.
    //
    // This supervisor path remains as a fallback for:
    //   - Helper worker process (worker_main -> vpn::start_with_password)
    //   - CLI invocations (exv start when helper is unavailable)
    //   - Direct fallback paths (darwin/linux try_connect_direct_fallback)
    //
    // When TunnelController IS active (e.g., if this function were called
    // while the desktop app is connected), we skip the supervisor and tell
    // the caller to use TunnelController instead.
    if (exv::core::is_tunnel_controller_active()) {
      logger::info("Native engine: TunnelController is active, skipping "
                   "supervisor spawn (caller should use Core-owned path)");
      timing.finish(false, "reason=tunnel_controller_active");
      return kUseTunnelController;
    }

    // TunnelController is not available — fall through to the legacy
    // supervisor path which owns the NativeVpnEngineSession lifecycle.
    logger::info("Native engine: TunnelController not available, using "
                 "supervisor fallback (legacy path)");

    auto validation = vpn_engine::validate_native_config(cfg);
    if (!validation.ok) {
      utils::print_error(validation.message);
      logger::error("Native VPN engine config invalid: " + validation.code);
      timing.finish(false, "reason=" + validation.code);
      return 1;
    }

    const pid_t process_pid = current_process_pid();
    pid_t existing_supervisor = read_supervisor_pid();
    if (existing_supervisor > 0 && existing_supervisor != process_pid &&
        is_process_alive(existing_supervisor)) {
      utils::print_warning("VPN is already running (supervisor PID: " +
                           std::to_string(existing_supervisor) + ")");
      utils::print_info("Use 'exv stop' to stop the current connection first.");
      timing.finish(false, "reason=already_running");
      return 1;
    }

    // LEGACY PATH: The native packet loop runs on a background thread owned
    // by the process hosting NativeVpnEngineSession. Spawn a durable
    // supervisor process to own the session so it outlives this short-lived
    // CLI/RPC invocation.
    int spawned_supervisor_pid = -1;
    if (!platform::spawn_vpn_supervisor_process(cfg, plaintext_password,
                                                retry_limit, run_supervisor,
                                                &spawned_supervisor_pid)) {
      utils::print_error("Failed to launch native VPN supervisor.");
      logger::error("Failed to spawn native VPN supervisor process");
      timing.finish(false, "reason=spawn_supervisor_failed");
      return 1;
    }
    write_supervisor_pid(static_cast<pid_t>(spawned_supervisor_pid));
    timing.mark("spawn_native_supervisor",
                "pid=" + std::to_string(spawned_supervisor_pid));

    // Wait for the supervisor-owned session to reach network-ready, sourcing
    // status from native-session-state.json (never the legacy route-ready
    // marker as the primary readiness signal).
    vpn_engine::NativeSessionProbe probe;
    probe.is_process_alive = [](int pid) {
      return platform::is_process_alive(pid);
    };
    for (int i = 0; i < 40; ++i) {
      sleep_ms(250);
      vpn_engine::NativeSessionSnapshot snapshot =
          vpn_engine::read_native_session_snapshot(utils::get_config_dir(),
                                                   probe);
      if (snapshot.network_ready) {
        std::cout << std::endl;
        utils::print_success("VPN connected successfully!");
        std::cout << utils::DIM << "  Supervisor PID: " << spawned_supervisor_pid
                  << utils::RESET << std::endl;
        std::cout << utils::DIM << "  Interface: " << snapshot.interface_name
                  << utils::RESET << std::endl;
        std::cout << utils::DIM << "  Internal IP: " << snapshot.internal_ip
                  << utils::RESET << std::endl;
        std::cout << utils::DIM << "  Server: " << cfg.server << utils::RESET
                  << std::endl;
        logger::info("Native VPN started under durable supervisor, PID: " +
                     std::to_string(spawned_supervisor_pid));
        timing.finish(true, "engine=native pid=" +
                                std::to_string(spawned_supervisor_pid));
        return 0;
      }
      if (!is_process_alive(spawned_supervisor_pid))
        break;
    }

    clear_runtime_state();
    // Do NOT clear native session state here: when called from the helper
    // worker process, handle_start() reads the failure_code from that file
    // after this function returns. Clearing here destroys the error before it
    // can be read and propagated to the desktop. handle_start() clears it
    // after reading. For direct CLI invocations the file is harmless because
    // the next connect attempt overwrites or clears it.
    utils::print_error("Native VPN session did not reach a network-ready state.");
    utils::print_info("Check logs with: exv logs");
    logger::error("Native VPN supervisor did not reach network-ready state");
    timing.finish(false, "reason=native_not_ready");
    return 1;
  }

  std::string openconnect_path = utils::get_openconnect_path(cfg.openconnect_runtime);
  timing.mark("resolve_openconnect",
              openconnect_path.empty() ? "result=missing" : "result=ok");
  if (openconnect_path.empty()) {
    utils::print_error("OpenConnect is not reachable by the current execution environment.");
    if (cfg.openconnect_runtime == "system") {
      utils::print_info("Install system openconnect or switch the runtime mode back to bundled/auto.");
    } else {
      utils::print_info("Ensure the desktop package contains the bundled OpenConnect runtime assets.");
    }
    logger::error("openconnect binary could not be resolved for VPN start");
    timing.finish(false, "reason=openconnect_missing");
    return 1;
  }

  // ── Generate tunnel script ─────────────────────────────────
  utils::print_info("Generating tunnel script...");
  if (!tunnel::write_script(cfg)) {
    timing.finish(false, "reason=tunnel_script_write_failed");
    return 1;
  }
  timing.mark("write_tunnel_script",
              "routes=" + std::to_string(cfg.routes.size()));
  utils::print_success("Tunnel script ready (" +
                       std::to_string(cfg.routes.size()) + " routes)");

  if (!utils::check_root()) {
#ifdef _WIN32
    utils::print_error("Administrator privileges required to start the VPN. Please run from an elevated prompt.");
    logger::error("Not running as Administrator for VPN start");
#else
    utils::print_error("Root privileges required to start the VPN. Please run with sudo.");
    logger::error("Not running as root for VPN start");
#endif
    timing.finish(false, "reason=not_elevated");
    return 1;
  }
  timing.mark("privilege_check", "result=ok");

  utils::print_info("Connecting to " + cfg.server + " ...");
  logger::info("Starting VPN: " + cfg.server + " user=" + cfg.username);
  append_openconnect_attempt_marker(cfg);
  AuthFailureWatch auth_watch(cfg);

  clear_runtime_state();
  timing.mark("clear_runtime_state");
  supervisor_stop_requested = 0;
  supervisor_child_pid = -1;

  bool use_supervisor = retry_limit != 0;
  pid_t supervisor_pid = -1;

  if (!use_supervisor) {
    ConnectionDiagnostics diagnostics("vpn.start.direct");
    pid_t child_pid = -1;
    platform::OpenconnectProcess child_process;
    if (!platform::spawn_openconnect_process(cfg, plaintext_password,
                                             &child_process)) {
      utils::print_error("Failed to launch openconnect process.");
      logger::error("Failed to spawn openconnect process");
      timing.finish(false, "reason=spawn_openconnect_failed");
      return 1;
    }
    child_pid = static_cast<pid_t>(child_process.pid);
    diagnostics.event("spawn_openconnect", "pid=" + std::to_string(child_pid));
    timing.mark("spawn_openconnect", "pid=" + std::to_string(child_pid));
#ifdef _WIN32
    platform::close_openconnect_process(&child_process);
#endif

    write_pid(child_pid);
    timing.mark("write_pid", "pid=" + std::to_string(child_pid));

    pid_t vpn_pid = -1;
    std::string vpn_interface;
    std::string internal_ip;
    bool route_ready = false;
    bool log_fallback_configured = false;
    for (int i = 0; i < 240; ++i) {
#ifndef _WIN32
      usleep(250000);
#else
      Sleep(250);
#endif
      vpn_pid = read_pid();
      route_ready = read_route_ready(&vpn_interface, &internal_ip);
#ifdef _WIN32
      if (!route_ready && !log_fallback_configured &&
          tunnel::configure_from_runtime_log(cfg)) {
        log_fallback_configured = true;
        route_ready = read_route_ready(&vpn_interface, &internal_ip);
        diagnostics.event("route_log_fallback",
                          route_ready ? "ready" : "pending");
        timing.mark("route_log_fallback",
                    route_ready ? "result=ready" : "result=pending");
      }
#endif
      if (!route_ready &&
          (auth_watch.failed() || tunnel::runtime_log_has_auth_failure(cfg))) {
        diagnostics.event("auth_failed");
        diagnostics.flush_summary(false);
        terminate_process(child_pid, true);
        clear_runtime_state();
        utils::print_error("VPN authentication failed or the server rejected the connection.");
        logger::error("OpenConnect reported authentication failure before network readiness");
        timing.finish(false, "reason=auth_failed");
        return kVpnInitialConnectFailedExitCode;
      }

      if (vpn_pid > 0 && !is_process_alive(vpn_pid)) {
        diagnostics.mark_process_exited_before_ready();
        diagnostics.flush_summary(false);
        clear_runtime_state();
        utils::print_error("Failed to establish the VPN connection.");
        utils::print_info("Check logs with: exv logs");
        logger::error("openconnect exited before initial connection was established");
        timing.finish(false, "reason=openconnect_exited_before_ready");
        return diagnostics.initial_connection_failed()
                   ? kVpnInitialConnectFailedExitCode
                   : 1;
      }

      if (vpn_pid > 0 && is_process_alive(vpn_pid) && route_ready) {
        diagnostics.mark_route_ready(vpn_interface, internal_ip);
        timing.mark("route_ready",
                    "pid=" + std::to_string(vpn_pid) + " interface=" +
                        vpn_interface + " internal_ip=" + internal_ip);
        break;
      }
    }

    if (vpn_pid > 0 && route_ready) {
      std::cout << std::endl;
      utils::print_success("VPN connected successfully!");
      std::cout << utils::DIM << "  PID: " << vpn_pid << utils::RESET
                << std::endl;
      std::cout << utils::DIM << "  Interface: " << vpn_interface << utils::RESET
                << std::endl;
      std::cout << utils::DIM << "  Internal IP: " << internal_ip
                << utils::RESET << std::endl;
      std::cout << utils::DIM << "  Server: " << cfg.server << utils::RESET
                << std::endl;
      std::cout << utils::DIM << "  Routes: " << cfg.routes.size()
                << " configured" << utils::RESET << std::endl;
      virtual_network::print_notice(
          virtual_network::detect_upstream_virtual_network(vpn_interface));
      std::cout << std::endl;
#ifdef _WIN32
      utils::print_info("Stop with: exv stop");
#else
      utils::print_info("Stop with: sudo exv stop");
#endif
      logger::info("VPN started, PID: " + std::to_string(vpn_pid));
      timing.finish(true, "pid=" + std::to_string(vpn_pid));
      diagnostics.flush_summary(true);
      return 0;
    }

    if (vpn_pid > 0 && is_process_alive(vpn_pid)) {
      terminate_process(vpn_pid, true);
    }
    clear_runtime_state();
    if (auth_watch.failed() || tunnel::runtime_log_has_auth_failure(cfg)) {
      utils::print_error("VPN authentication failed or the server rejected the connection.");
      logger::error("OpenConnect authentication failure detected after route-ready timeout");
      timing.finish(false, "reason=auth_failed_after_route_ready_timeout");
      diagnostics.event("auth_failed_after_timeout");
      diagnostics.flush_summary(false);
      return kVpnInitialConnectFailedExitCode;
    }
    utils::print_error("VPN process started, but network configuration did not complete.");
    utils::print_info("Check logs with: exv logs");
    logger::error("VPN start aborted because route-ready marker was not detected");
    timing.finish(false, "reason=route_ready_timeout");
    diagnostics.event("route_ready_timeout");
    diagnostics.flush_summary(false);
    return 1;
  }

  supervisor_pid = -1;
  int spawned_supervisor_pid = -1;
  if (!platform::spawn_vpn_supervisor_process(cfg, plaintext_password,
                                              retry_limit, run_supervisor,
                                              &spawned_supervisor_pid)) {
    utils::print_error("Failed to launch reconnect supervisor.");
    logger::error("Failed to spawn reconnect supervisor process");
    timing.finish(false, "reason=spawn_supervisor_failed");
    return 1;
  }
  supervisor_pid = static_cast<pid_t>(spawned_supervisor_pid);
  timing.mark("spawn_supervisor", "pid=" + std::to_string(supervisor_pid));

  write_supervisor_pid(supervisor_pid);
  timing.mark("write_supervisor_pid", "pid=" + std::to_string(supervisor_pid));

  pid_t vpn_pid = -1;
  std::string vpn_interface;
  std::string internal_ip;
  bool route_ready = false;
  bool log_fallback_configured = false;
  for (int i = 0; i < 20; ++i) {
#ifndef _WIN32
    usleep(250000);
#else
    Sleep(250);
#endif
    vpn_pid = read_pid();
    route_ready = read_route_ready(&vpn_interface, &internal_ip);
#ifdef _WIN32
    if (!route_ready && !log_fallback_configured &&
        tunnel::configure_from_runtime_log(cfg)) {
      log_fallback_configured = true;
      route_ready = read_route_ready(&vpn_interface, &internal_ip);
      timing.mark("route_log_fallback",
                  route_ready ? "result=ready" : "result=pending");
    }
#endif
    if (!route_ready &&
        (auth_watch.failed() || tunnel::runtime_log_has_auth_failure(cfg))) {
      if (vpn_pid > 0 && is_process_alive(vpn_pid))
        terminate_process(vpn_pid, true);
      if (supervisor_pid > 0 && is_process_alive(supervisor_pid))
        terminate_process(supervisor_pid, true);
      clear_runtime_state();
      utils::print_error("VPN authentication failed or the server rejected the connection.");
      logger::error("OpenConnect reported authentication failure before initial network readiness");
      timing.finish(false, "reason=auth_failed");
      return kVpnInitialConnectFailedExitCode;
    }
    if (vpn_pid > 0 && is_process_alive(vpn_pid) && route_ready) {
      timing.mark("route_ready",
                  "pid=" + std::to_string(vpn_pid) + " interface=" +
                      vpn_interface + " internal_ip=" + internal_ip);
      break;
    }
    if (!is_process_alive(supervisor_pid)) {
      clear_runtime_state();
      utils::print_error("Failed to establish the initial VPN connection.");
      utils::print_info("Check logs with: exv logs");
      logger::error("Reconnect supervisor exited before initial connection was established");
      timing.finish(false, "reason=supervisor_exited_before_ready");
      return 1;
    }
  }

  // Verify connection
  if (vpn_pid > 0 && route_ready) {
    std::cout << std::endl;
    utils::print_success("VPN connected successfully!");
    std::cout << utils::DIM << "  PID: " << vpn_pid << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Supervisor PID: " << supervisor_pid
              << utils::RESET << std::endl;
    std::cout << utils::DIM << "  Interface: " << vpn_interface << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Internal IP: " << internal_ip
              << utils::RESET << std::endl;
    std::cout << utils::DIM << "  Server: " << cfg.server << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Routes: " << cfg.routes.size()
              << " configured" << utils::RESET << std::endl;
    virtual_network::print_notice(
        virtual_network::detect_upstream_virtual_network(vpn_interface));
    if (retry_limit != 0) {
      std::cout << utils::DIM << "  Auto-reconnect: "
                << (retry_limit < 0 ? std::string("infinite")
                                    : std::to_string(retry_limit) + " retries")
                << utils::RESET << std::endl;
    }
    std::cout << std::endl;
#ifdef _WIN32
    utils::print_info("Stop with: exv stop");
#else
    utils::print_info("Stop with: sudo exv stop");
#endif
    logger::info("VPN started, PID: " + std::to_string(vpn_pid) +
                 ", supervisor PID: " + std::to_string(supervisor_pid));
    timing.finish(true, "pid=" + std::to_string(vpn_pid) +
                            " supervisor_pid=" + std::to_string(supervisor_pid));
  } else {
    if (vpn_pid > 0 && !is_process_alive(vpn_pid))
      vpn_pid = -1;

    utils::print_warning("VPN process started, but network routes are not ready yet.");
    if (vpn_pid > 0) {
      std::cout << utils::DIM << "  PID: " << vpn_pid << utils::RESET
                << std::endl;
    }
    std::cout << utils::DIM << "  Supervisor PID: " << supervisor_pid
              << utils::RESET << std::endl;
    std::cout << utils::DIM << "  Server: " << cfg.server << utils::RESET
              << std::endl;
    std::cout << utils::DIM << "  Routes: " << cfg.routes.size()
              << " configured" << utils::RESET << std::endl;
    std::cout << utils::DIM << "  Auto-reconnect: "
              << (retry_limit < 0 ? std::string("infinite")
                                  : std::to_string(retry_limit) + " retries")
              << utils::RESET << std::endl;
    std::cout << std::endl;
    utils::print_info("Check status with: exv status");
    utils::print_info("Check logs with: exv logs");
#ifdef _WIN32
    utils::print_info("Stop with: exv stop");
#else
    utils::print_info("Stop with: sudo exv stop");
#endif
    logger::warn("VPN supervisor started and returned before route-ready marker was detected");
    timing.finish(false, "reason=initial_route_ready_pending supervisor_pid=" +
                             std::to_string(supervisor_pid));
  }

  return 0;
}

bool stop_direct_session() {
  pid_t supervisor_pid = read_supervisor_pid();
  if (supervisor_pid > 0 && !is_process_alive(supervisor_pid)) {
    remove_supervisor_pid();
    supervisor_pid = -1;
  }

  pid_t pid = read_pid();
  if (pid <= 0 || !is_process_alive(pid)) {
    remove_pid();
    pid = find_openconnect_pid();
  }

  if (pid <= 0 && supervisor_pid <= 0) {
    clear_runtime_state();
    return true;
  }

// Clean up routes before killing openconnect — while the tunnel
  // interface is still valid, route deletion is more reliable.
  // On Windows, tunnel::cleanup_routes() is a no-op.
  tunnel::cleanup_routes();

  if (supervisor_pid > 0)
    terminate_process(supervisor_pid, false);
  if (pid > 0)
    terminate_process(pid, false);

  for (int i = 0; i < 10; ++i) {
    sleep_ms(300);
    if ((pid <= 0 || !is_process_alive(pid)) &&
        (supervisor_pid <= 0 || !is_process_alive(supervisor_pid))) {
      break;
    }
  }

  if (pid > 0 && is_process_alive(pid))
    terminate_process(pid, true);
  if (supervisor_pid > 0 && is_process_alive(supervisor_pid))
    terminate_process(supervisor_pid, true);

  if ((pid > 0 && is_process_alive(pid)) ||
      (supervisor_pid > 0 && is_process_alive(supervisor_pid))) {
    return false;
  }

  clear_runtime_state();
  return true;
}

int stop() {
  utils::print_header("EXV Stopping");

  // Prefer helper daemon even when running as root — it manages session state
  // and the reconnect supervisor. Direct kills leave stale state and the
  // supervisor respawns openconnect immediately.
  if (helper::is_available()) {
    bool ok = helper::stop_via_helper();
    if (ok) return 0;
    // Helper said not running — check for orphaned processes before giving up
  }

  if (!utils::check_root()) {
#ifdef _WIN32
    utils::print_error("Administrator privileges required. Please run from an elevated prompt.");
#else
    utils::print_error("Root privileges required. Please run with sudo.");
#endif
    return 1;
  }

  pid_t supervisor_pid = read_supervisor_pid();
  if (supervisor_pid > 0 && !is_process_alive(supervisor_pid)) {
    remove_supervisor_pid();
    supervisor_pid = -1;
  }

  // Find openconnect process
  pid_t pid = read_pid();
  if (pid <= 0 || !is_process_alive(pid)) {
    // Try pgrep as fallback
    pid = find_openconnect_pid();
  }

  if (pid <= 0 && supervisor_pid <= 0) {
    utils::print_error("No openconnect process found. VPN is not running.");
    clear_runtime_state();
    logger::info("Stop requested but no VPN process found");
    return 1;
  }

  if (pid > 0) {
    utils::print_info("Found openconnect process: PID " + std::to_string(pid));
  }
  if (supervisor_pid > 0) {
    utils::print_info("Found reconnect supervisor: PID " +
                      std::to_string(supervisor_pid));
  }
  utils::print_info("Stopping VPN...");
  logger::info("Stopping VPN, PID: " + std::to_string(pid) +
               ", supervisor PID: " + std::to_string(supervisor_pid));

  // Clean up routes before killing openconnect — while the tunnel
  // interface is still valid, route deletion is more reliable.
  // On Windows, tunnel::cleanup_routes() is a no-op.
  tunnel::cleanup_routes();

  if (supervisor_pid > 0)
    terminate_process(supervisor_pid, false);
  if (pid > 0)
    terminate_process(pid, false);

  for (int i = 0; i < 10; ++i) {
    sleep_ms(300);
    if ((pid <= 0 || !is_process_alive(pid)) &&
        (supervisor_pid <= 0 || !is_process_alive(supervisor_pid))) {
      break;
    }
  }

  if (pid > 0 && is_process_alive(pid)) {
    utils::print_warning("openconnect still running, force terminating...");
    terminate_process(pid, true);
  }
  if (supervisor_pid > 0 && is_process_alive(supervisor_pid)) {
    utils::print_warning("Reconnect supervisor still running, force terminating...");
    terminate_process(supervisor_pid, true);
  }

  sleep_ms(500);

  pid_t remaining_pid = find_openconnect_pid();
  if (remaining_pid > 0 && remaining_pid != pid) {
    utils::print_warning("Detected remaining openconnect process, force terminating...");
    terminate_process(remaining_pid, true);
    sleep_ms(500);
  }

  if ((pid > 0 && is_process_alive(pid)) ||
      (remaining_pid > 0 && is_process_alive(remaining_pid)) ||
      (supervisor_pid > 0 && is_process_alive(supervisor_pid))) {
    utils::print_error("Failed to stop VPN connection!");
    logger::error("Failed to stop VPN, PID: " + std::to_string(pid) +
                  ", remaining PID: " + std::to_string(remaining_pid) +
                  ", supervisor PID: " + std::to_string(supervisor_pid));
    return 1;
  }

  clear_runtime_state();
  std::cout << std::endl;
  utils::print_success("VPN connection stopped successfully! 🎉");
  logger::info("VPN stopped successfully");
  return 0;
}

int status() {
  utils::print_header("EXV Status");

  if (helper::is_available()) {
    return helper::show_status_via_helper() ? 0 : 1;
  }

  Config cfg = config::load();
  RuntimeStatusSnapshot snapshot = read_runtime_status_snapshot(cfg);

  if (snapshot.running) {
    std::cout << utils::GREEN << utils::BOLD << "  ● VPN is RUNNING"
              << utils::RESET << std::endl;
    std::cout << std::endl;
    if (snapshot.pid > 0) {
      std::cout << "  PID            : " << snapshot.pid;
      if (snapshot.pid_from_openconnect_scan)
        std::cout << "  (detected via pgrep)";
      std::cout << std::endl;
    }
    if (snapshot.supervisor_pid > 0)
      std::cout << "  Supervisor PID : " << snapshot.supervisor_pid
                << std::endl;
    std::cout << "  Network Ready  : "
              << (snapshot.network_ready ? "yes"
                                          : "no (waiting for tunnel script)")
              << std::endl;
    if (snapshot.network_ready) {
      std::cout << "  Interface      : " << snapshot.interface_name
                << std::endl;
      std::cout << "  Internal IP    : " << snapshot.internal_ip
                << std::endl;
    }

    if (!snapshot.interfaces_output.empty()) {
      std::cout << std::endl;
      std::cout << utils::DIM << "  Network Interfaces:" << utils::RESET
                << std::endl;
      std::istringstream iss(snapshot.interfaces_output);
      std::string line;
      while (std::getline(iss, line)) {
        std::cout << "    " << line << std::endl;
      }
    }
  } else {
    std::cout << utils::RED << utils::BOLD << "  ● VPN is NOT RUNNING"
              << utils::RESET << std::endl;
    clear_runtime_state();
  }

  std::cout << std::endl;
  return 0;
}

} // namespace vpn
} // namespace ecnuvpn
