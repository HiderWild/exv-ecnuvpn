#include "vpn.hpp"
#include "app_api.hpp"
#include "config.hpp"
#include "core/timing.hpp"
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

using ConnectTiming = exv::core::ConnectStageTimer;

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

    // Homebrew found �?attempt to install openconnect
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

  // Prefer helper daemon even when running as root �?it manages session state
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

// DEPRECATED: Legacy VPN start path.  New code should use
// TunnelController::connect() which delegates to CoreSessionRunner.
// This function is retained for the legacy CLI path only.
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

    // TunnelController is not available �?fall through to the legacy
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

// Clean up routes before killing openconnect �?while the tunnel
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

  // Prefer helper daemon even when running as root �?it manages session state
  // and the reconnect supervisor. Direct kills leave stale state and the
  // supervisor respawns openconnect immediately.
  if (helper::is_available()) {
    bool ok = helper::stop_via_helper();
    if (ok) return 0;
    // Helper said not running �?check for orphaned processes before giving up
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

  // Clean up routes before killing openconnect �?while the tunnel
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
    std::cout << utils::GREEN << utils::BOLD << "  �?VPN is RUNNING"
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
    std::cout << utils::RED << utils::BOLD << "  �?VPN is NOT RUNNING"
              << utils::RESET << std::endl;
    clear_runtime_state();
  }

  std::cout << std::endl;
  return 0;
}

} // namespace vpn
} // namespace ecnuvpn
