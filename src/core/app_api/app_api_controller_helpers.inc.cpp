
// =========================================================================
// D3: Legacy state-file cleanup for TunnelController path.
//
// When connecting via TunnelController (Core-owned mode), the in-memory
// TunnelStatusSnapshot replaces file-based persistence.  Any leftover
// native-session-state.json or ecnuvpn-supervisor.pid from a previous
// legacy-supervisor session must be cleaned up so crash recovery does not
// misinterpret stale state.  The read-side functions (load_native_session_state,
// read_native_session_snapshot) are preserved for crash recovery but the
// new architecture never writes these files.
// =========================================================================

void cleanup_legacy_supervisor_state_files() {
  const std::string config_dir = utils::get_config_dir();
  // Remove native-session-state.json and route-ready marker left by the
  // legacy NativeSessionEventRecorder.
  // Note: clear_native_session_state was removed in Phase 3A with the
  // native_session_store.hpp file. These files are no longer written.

  // Legacy supervisor PID file cleanup (no longer used).
  std::string supervisor_pid_file = platform::supervisor_pid_path(config_dir);
  if (utils::file_exists(supervisor_pid_file))
    std::remove(supervisor_pid_file.c_str());
}

// =========================================================================
// TunnelController singleton — lazily initialized on first VPN action.
// Holds the HelperClient, PlatformNetworkOps, and TunnelController as a
// single unit so their lifetimes are correctly managed.
// =========================================================================

struct TunnelControllerHolder {
  std::unique_ptr<exv::helper::HelperConnector> connector;
  std::shared_ptr<exv::helper::HelperClient> client;
  std::shared_ptr<exv::platform::HelperDelegatingPlatformNetworkOps> net_ops;
  std::shared_ptr<exv::core::TunnelController> controller;
  bool init_attempted = false;
  std::string init_error;
  // Cooldown to prevent tight retry loops (e.g. from status polling).
  // After a failed connection attempt, skip retries for this duration.
  std::chrono::steady_clock::time_point last_failure_time;
  static constexpr auto kRetryCooldown = std::chrono::seconds(30);
};

TunnelControllerHolder &tunnel_holder() {
  static TunnelControllerHolder holder;
  return holder;
}

/// Lazily create and connect the TunnelController. Returns nullptr if the
/// helper daemon cannot be reached. Enforces a cooldown between retries to
/// prevent tight reconnect loops from status polling or other frequent callers.
std::shared_ptr<exv::core::TunnelController> ensure_tunnel_controller(
    const std::string& endpoint_override = "") {
  auto &h = tunnel_holder();
  if (h.controller) {
    exv::core::set_tunnel_controller_active(true);
    return h.controller;
  }

  // Cooldown check: don't retry immediately after a recent failure.
  auto now = std::chrono::steady_clock::now();
  if (h.last_failure_time != std::chrono::steady_clock::time_point{}) {
    auto elapsed = std::chrono::duration_cast<std::chrono::seconds>(
        now - h.last_failure_time);
    if (elapsed < TunnelControllerHolder::kRetryCooldown) {
      // Still in cooldown — don't log spam, just return nullptr.
      return nullptr;
    }
  }

  h.init_attempted = true;
  try {
    h.connector = exv::helper::HelperConnector::create();
    exv::helper::HelperConnectorConfig cc;
    cc.mode = exv::helper::ConnectorMode::Transient;
    // When endpoint_override is provided (oneshot pipe path from backend
    // resolution), set it as the explicit pipe endpoint. Otherwise, record
    // the helper binary path for future oneshot launch and let the connector
    // fall through to the platform default endpoint.
    if (!endpoint_override.empty()) {
      cc.pipe_endpoint = endpoint_override;
    } else {
      cc.helper_executable_path = helper_binary_next_to_exv();
    }
    h.client = h.connector->connect(cc);
    if (!h.client) {
      h.init_error = "Failed to connect to helper daemon";
      h.last_failure_time = now;
      return nullptr;
    }
    h.net_ops =
        std::make_shared<exv::platform::HelperDelegatingPlatformNetworkOps>(
            h.client.get());
    h.controller = std::make_shared<exv::core::TunnelController>(h.client,
                                                                  h.net_ops);
    exv::core::set_tunnel_controller_active(true);
    return h.controller;
  } catch (const std::exception &e) {
    h.init_error = e.what();
    h.last_failure_time = now;
    return nullptr;
  }
}

/// Return the existing TunnelController if already initialized, or nullptr
/// without attempting to create one.
std::shared_ptr<exv::core::TunnelController> get_tunnel_controller_if_exists() {
  return tunnel_holder().controller;
}

/// Reset the TunnelController singleton, disconnecting any existing connection.
/// Use this before starting a new connection to ensure clean state.
void reset_tunnel_controller() {
  auto &h = tunnel_holder();
  if (h.controller) {
    h.controller.reset();
  }
  if (h.client) {
    h.client.reset();
  }
  if (h.net_ops) {
    h.net_ops.reset();
  }
  if (h.connector) {
    h.connector.reset();
  }
  h.init_attempted = false;
  h.init_error.clear();
  h.last_failure_time = std::chrono::steady_clock::time_point{};
  exv::core::set_tunnel_controller_active(false);
}

/// Map a TunnelStatusSnapshot to the frontend-compatible JSON format that
/// the WebUI expects. Field names and types match the legacy helper response.
nlohmann::json frontend_status_from_controller_snapshot(
    const exv::core::TunnelStatusSnapshot &snap, const Config &cfg) {
  bool running = snap.phase != exv::core::TunnelPhase::Idle &&
                 snap.phase != exv::core::TunnelPhase::Failed;
  bool connected = running && snap.network_ready;

  nlohmann::json j;
  j["connected"] = connected;
  j["process_running"] = running;
  j["server"] = snap.server.empty() ? cfg.server : snap.server;
  j["username"] = cfg.username;
  j["pid"] = -1;           // TunnelController does not expose raw PIDs
  j["network_ready"] = snap.network_ready;
  j["interface"] = snap.interface_name;
  j["internal_ip"] = "";
  j["route_count"] = static_cast<int>(cfg.routes.size());
  j["mtu"] = cfg.mtu;
  j["uptime_seconds"] = 0;
  j["rx_bytes"] = 0;
  j["tx_bytes"] = 0;
  if (snap.last_error.has_value()) {
    j["error"] = snap.last_error->message;
    j["error_code"] = snap.last_error->code;
    j["error_recoverable"] = snap.last_error->recoverable;
  }
  if (snap.reconnect.has_value()) {
    j["reconnect_attempt"] = snap.reconnect->attempt;
    j["reconnect_next_retry_ms"] = snap.reconnect->next_retry_ms;
  }
  j["auto_reconnect"] = snap.auto_reconnect;
  j["phase"] = [snap]() -> std::string {
    switch (snap.phase) {
    case exv::core::TunnelPhase::Idle: return "idle";
    case exv::core::TunnelPhase::PreparingHelper: return "preparing_helper";
    case exv::core::TunnelPhase::Authenticating: return "authenticating";
    case exv::core::TunnelPhase::ConnectingCstp: return "connecting_cstp";
    case exv::core::TunnelPhase::ApplyingNetworkConfig:
      return "applying_network_config";
    case exv::core::TunnelPhase::OpeningPacketDevice:
      return "opening_packet_device";
    case exv::core::TunnelPhase::Connected: return "connected";
    case exv::core::TunnelPhase::Reconnecting: return "reconnecting";
    case exv::core::TunnelPhase::Disconnecting: return "disconnecting";
    case exv::core::TunnelPhase::CleaningUp: return "cleaning_up";
    case exv::core::TunnelPhase::Failed: return "failed";
    default: return "unknown";
    }
  }();
  try {
    virtual_network::add_status_fields(j, snap.interface_name);
  } catch (...) {
  }
  return j;
}

nlohmann::json driver_status_json(const Config &cfg) {
  return platform::driver_status_json(cfg);
}

nlohmann::json install_driver(const Config &cfg, const nlohmann::json &payload) {
  return platform::install_driver(cfg, payload);
}

nlohmann::json preflight_connect(const Config &cfg, const std::string &password) {
  if (cfg.server.empty())
    return error("VPN server is not configured.");
  if (cfg.username.empty())
    return error("VPN username is not configured.");
  if (password.empty())
    return error("VPN password is not configured.");

  if (cfg.vpn_engine == "native") {
    auto native_validation = vpn_engine::validate_native_config(cfg);
    if (!native_validation.ok)
      return error(native_validation.message, native_validation.code);
  }

  platform::BackendResolveOptions options;
  options.preferred_mode = "auto";  // Auto-select: try service first, fallback to oneshot
  options.allow_oneshot = true;     // Allow oneshot mode when service unavailable
  options.start_oneshot = true;     // Start oneshot helper with UAC elevation if needed
  options.allow_service_start = false;
  options.helper_path = helper_binary_next_to_exv();  // Provide helper path for oneshot
  nlohmann::json backend = platform::resolve_backend(options);
  if (!backend.value("ok", false)) {
    return platform::backend_unavailable_error(
        backend, platform::helper_unavailable_connect_message());
  }

  nlohmann::json runtime = runtime_status_json(cfg);
  if (!runtime.value("available", false)) {
    return error("VPN runtime is not available. The desktop bundle is missing the selected VPN engine dependencies.");
  }

  nlohmann::json platform_err = platform::preflight_connect_platform_checks(cfg);
  if (platform_err.is_object() && platform_err.value("ok", true) == false)
    return platform_err;

  // Return success with backend information (including endpoint for oneshot)
  nlohmann::json result;
  result["ok"] = true;
  result["backend"] = backend;
  return result;
}

nlohmann::json logs_json(const nlohmann::json &payload) {
  config::ConfigManager mgr = make_config_manager();
  Config cfg = mgr.load();
  (void)cfg;
  // Read from the unified log path pinned by the runtime module, NOT from
  // cfg.log_file, so the UI sees exactly what every process writes and what
  // `exv logs` shows. Using cfg.log_file here previously diverged from the
  // real log location and made logs appear empty in the app.
  std::string log_path = runtime::paths().log_path;
  int max_lines = payload.value("lines", 100);
  if (max_lines < 1)
    max_lines = 1;
  if (max_lines > 10000)
    max_lines = 10000;
  std::string filter = payload.value("filter", std::string());

  nlohmann::json lines = nlohmann::json::array();
  std::vector<std::string> all_lines;
  std::ifstream ifs(log_path);
  std::string line;
  while (std::getline(ifs, line)) {
    if (!line.empty() && line.back() == '\r')
      line.pop_back();
    if (filter.empty() || line.find(filter) != std::string::npos)
      all_lines.push_back(line);
  }

  size_t start = all_lines.size() > static_cast<size_t>(max_lines)
                     ? all_lines.size() - static_cast<size_t>(max_lines)
                     : 0;
  for (size_t i = start; i < all_lines.size(); ++i) {
    lines.push_back({{"timestamp", ""},
                     {"level", "info"},
                     {"message", json_safe_text(all_lines[i])}});
  }
  return lines;
}
