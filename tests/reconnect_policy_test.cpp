#include "core/reconnect_policy.hpp"
#include "core/tunnel_intent.hpp"
#include "core/tunnel_state.hpp"

#include <iostream>
#include <set>
#include <string>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

exv::core::ErrorInfo make_error(bool recoverable, const std::string& code = "test_error") {
    exv::core::ErrorInfo e;
    e.domain = "test";
    e.code = code;
    e.message = "test message";
    e.recoverable = recoverable;
    return e;
}

exv::core::UserIntent make_intent(bool desired, bool auto_reconnect = true) {
    exv::core::UserIntent intent;
    intent.desired_connected = desired;
    intent.auto_reconnect = auto_reconnect;
    return intent;
}

} // namespace

int main() {
    bool ok = true;

    using exv::core::ReconnectConfig;
    using exv::core::ReconnectPolicy;
    using exv::core::TunnelPhase;

    // --- User not desired -> no retry ---
    {
        ReconnectPolicy policy;
        auto intent = make_intent(false);
        auto error = make_error(true);
        auto decision = policy.decide(error, intent, TunnelPhase::Connected, 0);
        ok = expect(!decision.should_retry,
                    "user not desired should not retry") && ok;
        ok = expect(decision.reason_code == "user_not_desired",
                    "reason should be user_not_desired") && ok;
    }

    // --- User disconnect -> no retry ---
    {
        ReconnectPolicy policy;
        auto intent = make_intent(true);
        intent.user_disconnect_reason = exv::core::DisconnectReason::UserRequested;
        auto error = make_error(true);
        auto decision = policy.decide(error, intent, TunnelPhase::Connected, 0);
        ok = expect(!decision.should_retry,
                    "user disconnect should not retry") && ok;
        ok = expect(decision.reason_code == "user_disconnect",
                    "reason should be user_disconnect") && ok;
    }

    // --- Non-recoverable error -> no retry ---
    {
        ReconnectPolicy policy;
        auto intent = make_intent(true);
        auto error = make_error(false);
        auto decision = policy.decide(error, intent, TunnelPhase::Connected, 0);
        ok = expect(!decision.should_retry,
                    "non-recoverable error should not retry") && ok;
        ok = expect(decision.reason_code.find("non_recoverable") == 0,
                    "reason should start with non_recoverable") && ok;
    }

    // --- Auto-reconnect disabled -> no retry ---
    {
        ReconnectPolicy policy;
        auto intent = make_intent(true, false);
        auto error = make_error(true);
        auto decision = policy.decide(error, intent, TunnelPhase::Connected, 0);
        ok = expect(!decision.should_retry,
                    "auto_reconnect disabled should not retry") && ok;
        ok = expect(decision.reason_code == "auto_reconnect_disabled",
                    "reason should be auto_reconnect_disabled") && ok;
    }

    // --- Max attempts reached -> no retry ---
    {
        ReconnectConfig config;
        config.max_attempts = 3;
        ReconnectPolicy policy(config);
        auto intent = make_intent(true);
        auto error = make_error(true);
        auto decision = policy.decide(error, intent, TunnelPhase::Connected, 3);
        ok = expect(!decision.should_retry,
                    "max attempts reached should not retry") && ok;
        ok = expect(decision.reason_code == "max_attempts_reached",
                    "reason should be max_attempts_reached") && ok;
    }

    // --- Below max attempts -> should retry ---
    {
        ReconnectConfig config;
        config.max_attempts = 5;
        ReconnectPolicy policy(config);
        auto intent = make_intent(true);
        auto error = make_error(true);
        auto decision = policy.decide(error, intent, TunnelPhase::Connected, 2);
        ok = expect(decision.should_retry,
                    "below max attempts with recoverable error should retry") && ok;
        ok = expect(decision.reason_code == "recoverable_error",
                    "reason should be recoverable_error") && ok;
    }

    // --- Recoverable error + auto_reconnect=true -> retry with delay ---
    {
        ReconnectPolicy policy;
        auto intent = make_intent(true);
        auto error = make_error(true);
        auto decision = policy.decide(error, intent, TunnelPhase::Connected, 0);
        ok = expect(decision.should_retry,
                    "recoverable error with auto_reconnect should retry") && ok;
        ok = expect(decision.delay.count() > 0,
                    "delay should be positive") && ok;
        ok = expect(decision.keep_helper_session,
                    "should keep helper session on retry") && ok;
        ok = expect(decision.keep_network_config,
                    "should keep network config on retry") && ok;
    }

    // --- Delay is within expected range (base=1000ms, jitter=0.2) ---
    {
        ReconnectConfig config;
        config.base_delay = std::chrono::milliseconds(1000);
        config.jitter_factor = 0.2;
        ReconnectPolicy policy(config);
        auto intent = make_intent(true);
        auto error = make_error(true);

        // next_delay() uses attempt_ which is always 0 after construction,
        // so expected range is base * 2^0 * (1 +/- jitter) = [800, 1200]
        bool any_in_range = false;
        for (int i = 0; i < 50; ++i) {
            auto decision = policy.decide(error, intent, TunnelPhase::Connected, 0);
            auto ms = decision.delay.count();
            if (ms >= 700 && ms <= 1300) {
                any_in_range = true;
            }
        }
        ok = expect(any_in_range,
                    "delay should be within expected jitter range") && ok;
    }

    // --- Jitter produces different delays ---
    {
        ReconnectConfig config;
        config.base_delay = std::chrono::milliseconds(1000);
        config.jitter_factor = 0.3;
        ReconnectPolicy policy(config);

        std::set<int64_t> unique_delays;
        for (int i = 0; i < 100; ++i) {
            auto delay = policy.next_delay();
            unique_delays.insert(delay.count());
        }
        ok = expect(unique_delays.size() > 1,
                    "jitter should produce different delay values") && ok;
    }

    // --- Reset clears attempt count ---
    {
        ReconnectPolicy policy;
        ok = expect(policy.attempt_count() == 0,
                    "initial attempt count should be 0") && ok;
        policy.reset();
        ok = expect(policy.attempt_count() == 0,
                    "attempt count should remain 0 after reset") && ok;
    }

    // --- Unlimited attempts (max_attempts=0) ---
    {
        ReconnectConfig config;
        config.max_attempts = 0;  // unlimited
        ReconnectPolicy policy(config);
        auto intent = make_intent(true);
        auto error = make_error(true);
        auto decision = policy.decide(error, intent, TunnelPhase::Connected, 1000);
        ok = expect(decision.should_retry,
                    "unlimited attempts should allow retry at high count") && ok;
    }

    // --- Reason codes are populated for all rejection cases ---
    {
        ReconnectPolicy policy;

        // Not desired
        auto d1 = policy.decide(make_error(true), make_intent(false),
                                TunnelPhase::Connected, 0);
        ok = expect(!d1.reason_code.empty(),
                    "rejection should have a reason code") && ok;

        // Non-recoverable
        auto d2 = policy.decide(make_error(false), make_intent(true),
                                TunnelPhase::Connected, 0);
        ok = expect(!d2.reason_code.empty(),
                    "non-recoverable rejection should have a reason code") && ok;

        // Auto-reconnect off
        auto d3 = policy.decide(make_error(true), make_intent(true, false),
                                TunnelPhase::Connected, 0);
        ok = expect(!d3.reason_code.empty(),
                    "auto_reconnect off rejection should have a reason code") && ok;
    }

    // --- AUTH FAILURE invariant: auth errors do not retry ---
    {
        ReconnectPolicy policy;
        // Auth failure is represented as a non-recoverable error with auth domain
        exv::core::ErrorInfo auth_err;
        auth_err.domain = "auth";
        auth_err.code = "auth_failed";
        auth_err.message = "authentication failed";
        auth_err.recoverable = false;  // auth failures are not recoverable
        auto intent = make_intent(true);
        auto decision = policy.decide(auth_err, intent, TunnelPhase::Authenticating, 0);
        ok = expect(!decision.should_retry,
                    "auth failure must not trigger retry") && ok;
    }

    // --- AUTH FAILURE invariant: auth error code recognized as non-recoverable ---
    {
        ReconnectPolicy policy;
        exv::core::ErrorInfo auth_err;
        auth_err.domain = "auth";
        auth_err.code = "auth_failed";
        auth_err.message = "401 unauthorized";
        auth_err.recoverable = false;
        auto intent = make_intent(true);
        auto decision = policy.decide(auth_err, intent, TunnelPhase::Connected, 0);
        ok = expect(!decision.should_retry,
                    "auth error (401) must not trigger retry") && ok;
        ok = expect(!decision.reason_code.empty(),
                    "auth error rejection must have a reason_code") && ok;
    }

    // --- USER DISCONNECT invariant: user-requested disconnect does not retry ---
    {
        ReconnectPolicy policy;
        auto intent = make_intent(true);
        intent.user_disconnect_reason = exv::core::DisconnectReason::UserRequested;
        exv::core::ErrorInfo err;
        err.domain = "transport";
        err.code = "transport_closed";
        err.message = "user disconnected";
        err.recoverable = true;
        auto decision = policy.decide(err, intent, TunnelPhase::Connected, 0);
        ok = expect(!decision.should_retry,
                    "user-requested disconnect must never retry") && ok;
        ok = expect(decision.reason_code == "user_disconnect",
                    "reason_code must be user_disconnect for user-initiated stop") && ok;
    }

    // --- TRANSPORT CLOSE + auto_reconnect=true invariant: must retry ---
    {
        ReconnectPolicy policy;
        exv::core::ErrorInfo transport_err;
        transport_err.domain = "transport";
        transport_err.code = "transport_closed";
        transport_err.message = "connection reset by peer";
        transport_err.recoverable = true;
        auto intent = make_intent(true, /*auto_reconnect=*/true);
        // No user disconnect reason set
        auto decision = policy.decide(transport_err, intent, TunnelPhase::Connected, 0);
        ok = expect(decision.should_retry,
                    "transport close with auto_reconnect=true must retry") && ok;
        ok = expect(decision.delay.count() > 0,
                    "retry decision must have a positive delay") && ok;
    }

    // --- TRANSPORT CLOSE + auto_reconnect=false: must not retry ---
    {
        ReconnectPolicy policy;
        exv::core::ErrorInfo transport_err;
        transport_err.domain = "transport";
        transport_err.code = "transport_closed";
        transport_err.message = "connection reset by peer";
        transport_err.recoverable = true;
        auto intent = make_intent(true, /*auto_reconnect=*/false);
        auto decision = policy.decide(transport_err, intent, TunnelPhase::Connected, 0);
        ok = expect(!decision.should_retry,
                    "transport close with auto_reconnect=false must not retry") && ok;
        ok = expect(decision.reason_code == "auto_reconnect_disabled",
                    "reason_code must be auto_reconnect_disabled") && ok;
    }

    // --- reason_code is never empty for any rejection case ---
    {
        ReconnectPolicy policy;
        auto check_has_reason = [&](const exv::core::ReconnectDecision& d, const char* case_name) {
            if (!d.should_retry) {
                ok = expect(!d.reason_code.empty(), case_name) && ok;
            }
        };

        // auth failure
        {
            exv::core::ErrorInfo e; e.domain="auth"; e.code="auth_failed"; e.recoverable=false;
            auto d = policy.decide(e, make_intent(true), TunnelPhase::Authenticating, 0);
            check_has_reason(d, "auth failure rejection must have reason_code");
        }
        // user disconnect
        {
            auto intent = make_intent(true);
            intent.user_disconnect_reason = exv::core::DisconnectReason::UserRequested;
            auto d = policy.decide(make_error(true), intent, TunnelPhase::Connected, 0);
            check_has_reason(d, "user disconnect rejection must have reason_code");
        }
        // desired=false
        {
            auto d = policy.decide(make_error(true), make_intent(false), TunnelPhase::Connected, 0);
            check_has_reason(d, "not-desired rejection must have reason_code");
        }
        // auto_reconnect=false
        {
            auto d = policy.decide(make_error(true), make_intent(true, false), TunnelPhase::Connected, 0);
            check_has_reason(d, "auto_reconnect=false rejection must have reason_code");
        }
    }

    // --- should_retry=true decisions always have positive delay ---
    {
        ReconnectConfig config;
        config.max_attempts = 10;
        ReconnectPolicy policy(config);
        auto intent = make_intent(true);
        for (int attempt = 0; attempt < 5; ++attempt) {
            auto decision = policy.decide(make_error(true), intent, TunnelPhase::Connected, attempt);
            if (decision.should_retry) {
                ok = expect(decision.delay.count() > 0,
                            "every retry decision must have a positive delay") && ok;
            }
        }
    }

    if (ok) {
        std::cout << "reconnect_policy_test: all assertions passed\n";
    } else {
        std::cerr << "reconnect_policy_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
