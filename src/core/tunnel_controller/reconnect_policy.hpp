#pragma once
#include <chrono>
#include <functional>
#include <string>
#include "tunnel_intent.hpp"
#include "tunnel_state.hpp"

namespace exv::core {

// Clock abstraction for deterministic testing
using ClockFunc = std::function<std::chrono::steady_clock::time_point()>;
using RandomFunc = std::function<double()>;  // Returns [0.0, 1.0)

struct ReconnectDecision {
    bool should_retry = false;
    std::chrono::milliseconds delay{0};
    std::string reason_code;
    bool keep_helper_session = false;
    bool keep_network_config = false;
};

struct ReconnectConfig {
    std::chrono::milliseconds base_delay{1000};
    std::chrono::milliseconds max_delay{60000};
    double jitter_factor = 0.2;  // plus/minus 20%
    std::chrono::seconds stable_reset_duration{60};
    int max_attempts = 0;  // 0 = unlimited
    ClockFunc clock;       // nullptr = use steady_clock::now()
    RandomFunc random;     // nullptr = use random_device
};

class ReconnectPolicy {
public:
    explicit ReconnectPolicy(ReconnectConfig config = {});

    // Core decision method - takes structured error + user intent
    ReconnectDecision decide(
        const ErrorInfo& error,
        const UserIntent& intent,
        TunnelPhase current_phase,
        int attempt_count
    ) const;

    // Reset attempt counter (called after stable connection)
    void reset();

    // Get current attempt count
    int attempt_count() const;

    // Calculate delay with jitter for current attempt
    std::chrono::milliseconds next_delay() const;

    // Get current time using injected or real clock
    std::chrono::steady_clock::time_point now() const;

private:
    ReconnectConfig config_;
    int attempt_ = 0;
};

} // namespace exv::core
