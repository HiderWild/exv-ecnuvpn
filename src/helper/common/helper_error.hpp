#pragma once
#include <string>
#include <optional>

namespace exv::helper {

enum class HelperErrorCode {
    None = 0,
    UnsupportedOp = 1,
    ProtocolRejected = 2,
    InvalidSession = 3,
    PermissionDenied = 4,
    DeviceNotFound = 5,
    DeviceBusy = 6,
    RouteApplyFailed = 7,
    DnsApplyFailed = 8,
    CleanupPartial = 9,
    InternalError = 100
};

struct HelperError {
    HelperErrorCode code = HelperErrorCode::None;
    std::string message;
    std::optional<int> native_code;
    std::string native_api;
};

} // namespace exv::helper
