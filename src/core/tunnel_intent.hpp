#pragma once
#include <optional>
#include <string>

namespace exv::core {

struct ProfileId {
    std::string value;
};

enum class DisconnectReason {
    UserRequested,
    AuthFailed,
    CertError,
    TransportClosed,
    HelperLost,
    PacketDeviceFailed,
    NetworkConfigFailed,
    LeaseExpired
};

struct UserIntent {
    bool desired_connected = false;
    bool auto_reconnect = true;
    ProfileId profile_id;
    std::optional<DisconnectReason> user_disconnect_reason;
};

} // namespace exv::core
