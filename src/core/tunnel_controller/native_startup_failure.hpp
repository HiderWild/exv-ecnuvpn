#pragma once
#include <string>
#include <optional>
#include "tunnel_state.hpp"

namespace exv::core {

// Classifies startup failures vs runtime failures
enum class FailurePhase {
    PreConnect,      // Before any connection attempt
    Authenticating,  // During auth
    Connecting,      // During CSTP/TLS connect
    StableReady,     // After stable connection established
    Unknown
};

struct StartupFailureInfo {
    FailurePhase phase = FailurePhase::Unknown;
    ErrorInfo error;
    bool is_terminal = false;      // true = don't retry
    bool is_recoverable = false;   // true = can retry
    std::string detail;            // Human-readable detail
};

class NativeStartupFailureAnalyzer {
public:
    // Classify a failure based on when it occurred
    static StartupFailureInfo classify(
        FailurePhase phase,
        const ErrorInfo& error,
        bool was_stable_ready
    );

    // Determine if failure is terminal (never retry)
    static bool is_terminal(const ErrorInfo& error);

    // Determine if failure is recoverable (can retry)
    static bool is_recoverable(const ErrorInfo& error, bool was_stable_ready);

    // Map native error codes to structured error
    static ErrorInfo map_native_error(
        int native_code,
        const std::string& api,
        FailurePhase phase
    );
};

} // namespace exv::core
