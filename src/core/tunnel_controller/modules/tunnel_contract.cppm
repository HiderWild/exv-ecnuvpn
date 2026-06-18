export module exv.core.tunnel.contract;

export namespace exv::core::tunnel::contract {

struct PhaseContract {
  const char *name;
  const char *wire_name;
  bool running;
  bool connected;
  bool network_ready;
};

inline constexpr PhaseContract PHASES[] = {
    {"Idle", "idle", false, false, false},
    {"PreparingHelper", "preparing_helper", true, false, false},
    {"Authenticating", "authenticating", true, false, false},
    {"ConnectingCstp", "connecting_cstp", true, false, false},
    {"ApplyingNetworkConfig", "applying_network_config", true, false, false},
    {"OpeningPacketDevice", "opening_packet_device", true, false, false},
    {"Connected", "connected", true, true, true},
    {"Reconnecting", "reconnecting", true, false, false},
    {"Disconnecting", "disconnecting", true, false, false},
    {"CleaningUp", "cleaning_up", true, false, false},
    {"Failed", "failed", false, false, false},
};

inline constexpr const char *EVENTS[] = {
    "UserConnect",
    "UserDisconnect",
    "SetAutoReconnect",
    "HelperReady",
    "AuthSucceeded",
    "AuthFailed",
    "AuthChallengeRequired",
    "AuthGroupRequired",
    "CstpConnected",
    "NetworkConfigApplied",
    "PacketLoopStarted",
    "TransportClosed",
    "PacketDeviceFailed",
    "HelperLost",
    "LeaseExpired",
    "ReconnectTimerFired",
    "CleanupSucceeded",
    "CleanupFailed",
};

inline constexpr const char *DISCONNECT_REASONS[] = {
    "UserRequested",
    "AuthFailed",
    "CertError",
    "TransportClosed",
    "HelperLost",
    "PacketDeviceFailed",
    "NetworkConfigFailed",
    "LeaseExpired",
};

inline constexpr const char *ERROR_DOMAINS[] = {
    "transport",
    "auth",
    "helper",
    "os.route",
    "os.dns",
    "packet",
    "config",
    "native",
};

inline constexpr const char *STATUS_FIELDS[] = {
    "phase",
    "desired_connected",
    "auto_reconnect",
    "helper_mode",
    "helper_status",
    "network_ready",
    "server",
    "interface_name",
    "last_error",
    "reconnect",
};

constexpr bool string_equal(const char *left, const char *right) noexcept {
  if (left == nullptr || right == nullptr) {
    return left == right;
  }
  while (*left != '\0' && *right != '\0') {
    if (*left != *right) {
      return false;
    }
    ++left;
    ++right;
  }
  return *left == *right;
}

constexpr unsigned phase_count() noexcept {
  return sizeof(PHASES) / sizeof(PHASES[0]);
}

constexpr unsigned event_count() noexcept {
  return sizeof(EVENTS) / sizeof(EVENTS[0]);
}

constexpr unsigned disconnect_reason_count() noexcept {
  return sizeof(DISCONNECT_REASONS) / sizeof(DISCONNECT_REASONS[0]);
}

constexpr unsigned error_domain_count() noexcept {
  return sizeof(ERROR_DOMAINS) / sizeof(ERROR_DOMAINS[0]);
}

constexpr unsigned status_field_count() noexcept {
  return sizeof(STATUS_FIELDS) / sizeof(STATUS_FIELDS[0]);
}

constexpr const PhaseContract *phase_contract(const char *name) noexcept {
  for (const auto &candidate : PHASES) {
    if (string_equal(candidate.name, name)) {
      return &candidate;
    }
  }
  return nullptr;
}

constexpr bool is_phase(const char *name) noexcept {
  return phase_contract(name) != nullptr;
}

constexpr bool is_event(const char *name) noexcept {
  for (const auto candidate : EVENTS) {
    if (string_equal(candidate, name)) {
      return true;
    }
  }
  return false;
}

constexpr bool is_disconnect_reason(const char *name) noexcept {
  for (const auto candidate : DISCONNECT_REASONS) {
    if (string_equal(candidate, name)) {
      return true;
    }
  }
  return false;
}

constexpr bool is_error_domain(const char *name) noexcept {
  for (const auto candidate : ERROR_DOMAINS) {
    if (string_equal(candidate, name)) {
      return true;
    }
  }
  return false;
}

constexpr bool is_status_field(const char *name) noexcept {
  for (const auto candidate : STATUS_FIELDS) {
    if (string_equal(candidate, name)) {
      return true;
    }
  }
  return false;
}

} // namespace exv::core::tunnel::contract
