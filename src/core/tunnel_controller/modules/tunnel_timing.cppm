export module exv.core.tunnel.timing;

export namespace exv::core::tunnel::timing {

inline constexpr const char *HELPER_PREPARE = "helper_prepare";
inline constexpr const char *AUTH = "auth";
inline constexpr const char *CSTP_CONNECT = "cstp_connect";
inline constexpr const char *NETWORK_CONFIG = "network_config";
inline constexpr const char *PACKET_DEVICE = "packet_device";
inline constexpr const char *FIRST_PACKET = "first_packet";

inline constexpr const char *CONNECT_STAGES[] = {
    HELPER_PREPARE,
    AUTH,
    CSTP_CONNECT,
    NETWORK_CONFIG,
    PACKET_DEVICE,
    FIRST_PACKET,
};

constexpr unsigned connect_stage_count() noexcept {
  return sizeof(CONNECT_STAGES) / sizeof(CONNECT_STAGES[0]);
}

constexpr const char *connect_stage_name(unsigned index) noexcept {
  return index < connect_stage_count() ? CONNECT_STAGES[index] : nullptr;
}

} // namespace exv::core::tunnel::timing
