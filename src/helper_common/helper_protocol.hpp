#pragma once
#include <cstdint>
#include <string>

namespace exv::helper {

constexpr uint32_t PROTOCOL_VERSION = 2;

enum class HelperOp : uint32_t {
    Hello = 1,
    StartSession = 2,
    PrepareTunnelDevice = 3,
    ApplyTunnelConfig = 4,
    Heartbeat = 5,
    Cleanup = 6,
    GetSnapshot = 7,
    EndSession = 8,
    // Legacy ops (backward compat)
    LegacyStart = 100,
    LegacyStop = 101,
    LegacyStatus = 102,
    LegacyHeartbeat = 103
};

enum class HelperMode : uint32_t {
    Transient = 1,
    Resident = 2
};

} // namespace exv::helper
