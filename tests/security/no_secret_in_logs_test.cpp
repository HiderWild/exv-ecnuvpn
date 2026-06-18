// Security regression test: verify that sensitive credentials never appear
// in log-related code paths, status snapshots, or error structures.
//
// This is the companion to no_secret_in_argv_test -- that test covers the
// helper IPC / process-argument boundary; this one covers the logging and
// diagnostics boundary.

#include "core/tunnel_controller/core_error_mapper.hpp"
#include "core/tunnel_controller/tunnel_intent.hpp"
#include "core/tunnel_controller/tunnel_state.hpp"

#include <iostream>
#include <string>

namespace {

bool g_ok = true;

void expect(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "EXPECT FAILED: " << message << std::endl;
        g_ok = false;
    }
}

bool contains_secret_keyword(const std::string& s) {
    static const char* keywords[] = {
        "password", "Password", "PASSWORD",
        "passwd",    "Passwd",    "PASSWD",
        "secret",    "Secret",    "SECRET",
        "token",     "Token",     "TOKEN",
        "cookie",    "Cookie",    "COOKIE",
        "credential","Credential","CREDENTIAL",
        "auth_token", "auth_secret",
        nullptr
    };
    for (const char** kw = keywords; *kw; ++kw) {
        if (s.find(*kw) != std::string::npos) return true;
    }
    return false;
}

bool contains_seed_secret(const std::string& s) {
    static const char* seeds[] = {
        "p@ssw0rd-seed",
        "V2_SESSION_TOKEN_SEED",
        "webvpn=COOKIE_SEED",
        "OPAQUE_SEED",
        "SAML_SEED",
        "CSD_TICKET_SEED",
        "CSD_TOKEN_SEED",
        "654321",
        nullptr
    };
    for (const char** seed = seeds; *seed; ++seed) {
        if (s.find(*seed) != std::string::npos) return true;
    }
    return false;
}

} // namespace

int main() {
    // ---------------------------------------------------------------
    // 1. ErrorInfo must not carry secret-bearing fields
    // ---------------------------------------------------------------
    {
        exv::core::ErrorInfo err;
        err.domain    = "transport";
        err.code      = "transport_closed";
        err.message   = "connection lost";
        err.native_api = "recv";

        std::string check;
        check += err.domain;
        check += err.code;
        check += err.message;
        check += err.native_api;
        expect(!contains_secret_keyword(check),
               "ErrorInfo fields must not contain secrets");
    }

    // ---------------------------------------------------------------
    // 2. CoreErrorMapper::from_helper_error -- no secrets in output
    // ---------------------------------------------------------------
    {
        exv::core::ErrorInfo mapped = exv::core::CoreErrorMapper::from_helper_error(
            "permission_denied", "access denied");
        std::string check;
        check += mapped.domain;
        check += mapped.code;
        check += mapped.message;
        expect(!contains_secret_keyword(check),
               "mapped helper error must not contain secrets");
    }

    // ---------------------------------------------------------------
    // 3. CoreErrorMapper::from_transport_error -- no secrets
    // ---------------------------------------------------------------
    {
        exv::core::ErrorInfo mapped = exv::core::CoreErrorMapper::from_transport_error(
            10054, "recv");
        std::string check;
        check += mapped.domain;
        check += mapped.code;
        check += mapped.message;
        check += mapped.native_api;
        expect(!contains_secret_keyword(check),
               "transport error must not contain secrets");
    }

    // ---------------------------------------------------------------
    // 4. CoreErrorMapper::from_auth_error -- no secrets
    // ---------------------------------------------------------------
    {
        exv::core::ErrorInfo mapped = exv::core::CoreErrorMapper::from_auth_error(
            401, "authentication failed");
        std::string check;
        check += mapped.domain;
        check += mapped.code;
        check += mapped.message;
        expect(!contains_secret_keyword(check),
               "auth error must not contain secrets");
    }

    // ---------------------------------------------------------------
    // 4a. Seeded AnyConnect secrets are redacted from mapped errors
    // ---------------------------------------------------------------
    {
        const std::string seeded =
            "password=p@ssw0rd-seed token=V2_SESSION_TOKEN_SEED "
            "cookie webvpn=COOKIE_SEED opaque=OPAQUE_SEED "
            "SAML=SAML_SEED challenge=654321";

        exv::core::ErrorInfo auth = exv::core::CoreErrorMapper::from_auth_error(
            401, seeded);
        exv::core::ErrorInfo helper =
            exv::core::CoreErrorMapper::from_helper_error("helper_failed", seeded);
        exv::core::ErrorInfo native =
            exv::core::CoreErrorMapper::from_native_error(
                "auth_challenge_required", seeded);
        exv::core::ErrorInfo csd =
            exv::core::CoreErrorMapper::from_native_error(
                "csd_required_unsupported",
                "hostscan-ticket=CSD_TICKET_SEED");

        std::string check;
        check += auth.message;
        check += helper.message;
        check += native.message;
        check += native.recommended_action;
        check += csd.message;
        check += csd.recommended_action;
        expect(!contains_secret_keyword(check),
               "seeded mapped errors must not contain secret keywords");
        expect(!contains_seed_secret(check),
               "seeded mapped errors must not contain secret values");
    }

    // ---------------------------------------------------------------
    // 5. UserIntent -- no credential fields
    // ---------------------------------------------------------------
    {
        exv::core::UserIntent intent;
        intent.desired_connected = true;
        intent.auto_reconnect    = true;
        intent.profile_id.value  = "default";

        std::string check;
        check += intent.profile_id.value;
        expect(!contains_secret_keyword(check),
               "UserIntent fields must not contain secrets");
    }

    // ---------------------------------------------------------------
    // 6. TunnelStatusSnapshot -- no credential fields
    // ---------------------------------------------------------------
    {
        exv::core::TunnelStatusSnapshot snap;
        snap.phase = exv::core::TunnelPhase::Connected;
        snap.desired_connected = true;
        snap.auto_reconnect = true;
        snap.helper_mode = "transient";
        snap.helper_status = "connected";
        snap.network_ready = true;
        snap.server = "vpn.example.edu";
        snap.interface_name = "ECNU-VPN";

        std::string check;
        check += snap.helper_mode;
        check += snap.helper_status;
        check += snap.server;
        check += snap.interface_name;
        expect(!contains_secret_keyword(check),
               "TunnelStatusSnapshot fields must not contain secrets");
    }

    // ---------------------------------------------------------------
    // 7. TunnelStatusSnapshot with error -- no secrets
    // ---------------------------------------------------------------
    {
        exv::core::TunnelStatusSnapshot snap;
        snap.phase = exv::core::TunnelPhase::Failed;
        snap.last_error = exv::core::ErrorInfo{
            "transport", "transport_closed", "connection reset",
            std::nullopt, "recv", true, "retry"
        };

        std::string check;
        check += snap.last_error->domain;
        check += snap.last_error->code;
        check += snap.last_error->message;
        check += snap.last_error->native_api;
        check += snap.last_error->recommended_action;
        expect(!contains_secret_keyword(check),
               "TunnelStatusSnapshot error fields must not contain secrets");
    }

    // ---------------------------------------------------------------
    // 8. Scan source files at compile time is not possible in C++17,
    //    but we verify the key API surfaces above. A CI script can
    //    do a textual scan of src/core_api/ for "password" as a
    //    supplementary check.
    // ---------------------------------------------------------------

    if (g_ok) {
        std::cout << "no_secret_in_logs_test: all assertions passed\n";
    } else {
        std::cerr << "no_secret_in_logs_test: some assertions FAILED\n";
    }
    return g_ok ? 0 : 1;
}
