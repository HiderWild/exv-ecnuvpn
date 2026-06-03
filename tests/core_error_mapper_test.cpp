// Tests for CoreErrorMapper: verifies error classification and recoverability.

#include "core/core_error_mapper.hpp"
#include "core/tunnel_state.hpp"

#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

} // namespace

int main() {
    bool ok = true;

    using exv::core::CoreErrorMapper;
    using exv::core::ErrorInfo;

    // --- from_transport_error: creates recoverable transport error ---
    {
        ErrorInfo err = CoreErrorMapper::from_transport_error(10054, "recv");
        ok = expect(err.domain == "transport",
                    "transport error domain should be transport") && ok;
        ok = expect(err.code == "transport_closed",
                    "transport error code should be transport_closed") && ok;
        ok = expect(err.recoverable,
                    "transport errors should be recoverable") && ok;
        ok = expect(err.native_code.has_value() && *err.native_code == 10054,
                    "native_code should be preserved") && ok;
        ok = expect(err.native_api == "recv",
                    "native_api should be preserved") && ok;
        ok = expect(!err.recommended_action.empty(),
                    "recommended_action should be populated") && ok;
    }

    // --- from_auth_error: creates non-recoverable auth error ---
    {
        ErrorInfo err = CoreErrorMapper::from_auth_error(401, "invalid credentials");
        ok = expect(err.domain == "auth",
                    "auth error domain should be auth") && ok;
        ok = expect(err.code == "auth_failed",
                    "auth error code should be auth_failed") && ok;
        ok = expect(!err.recoverable,
                    "auth errors should not be recoverable") && ok;
        ok = expect(err.message.find("invalid credentials") != std::string::npos,
                    "auth error message should include detail") && ok;
    }

    // --- from_helper_error: version_mismatch is not recoverable ---
    {
        ErrorInfo err = CoreErrorMapper::from_helper_error(
            "helper_version_mismatch", "helper version too old");
        ok = expect(err.domain == "helper",
                    "helper error domain should be helper") && ok;
        ok = expect(err.code == "helper_version_mismatch",
                    "error code should match") && ok;
        ok = expect(!err.recoverable,
                    "version_mismatch should not be recoverable") && ok;
    }

    // --- from_helper_error: other helper errors are recoverable ---
    {
        ErrorInfo err = CoreErrorMapper::from_helper_error(
            "helper_timeout", "helper did not respond");
        ok = expect(err.domain == "helper",
                    "helper error domain should be helper") && ok;
        ok = expect(err.recoverable,
                    "helper_timeout should be recoverable") && ok;
    }

    // --- from_platform_error: creates non-recoverable platform error ---
    {
        ErrorInfo err = CoreErrorMapper::from_platform_error(
            "os.route", 2, "CreateIpForwardEntry");
        ok = expect(err.domain == "os.route",
                    "platform error domain should be os.route") && ok;
        ok = expect(err.code == "os.route_failed",
                    "platform error code should be domain + _failed") && ok;
        ok = expect(!err.recoverable,
                    "platform errors should not be recoverable") && ok;
        ok = expect(err.native_code.has_value() && *err.native_code == 2,
                    "native_code should be preserved") && ok;
        ok = expect(err.native_api == "CreateIpForwardEntry",
                    "native_api should be preserved") && ok;
    }

    // --- is_recoverable: correctly classifies errors ---
    {
        ErrorInfo recoverable;
        recoverable.recoverable = true;
        ok = expect(CoreErrorMapper::is_recoverable(recoverable),
                    "should report recoverable errors as recoverable") && ok;

        ErrorInfo non_recoverable;
        non_recoverable.recoverable = false;
        ok = expect(!CoreErrorMapper::is_recoverable(non_recoverable),
                    "should report non-recoverable errors as not recoverable") && ok;
    }

    // --- ErrorInfo fields are properly set ---
    {
        ErrorInfo err = CoreErrorMapper::from_transport_error(0, "send");
        ok = expect(!err.message.empty(),
                    "error message should not be empty") && ok;
        ok = expect(!err.recommended_action.empty(),
                    "recommended_action should not be empty") && ok;
    }

    if (ok) {
        std::cout << "core_error_mapper_test: all assertions passed\n";
    } else {
        std::cerr << "core_error_mapper_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
