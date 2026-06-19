// Tests for CoreErrorMapper: verifies error classification and recoverability.

#include <iostream>
#include <string>

import exv.core.tunnel.errors;

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

bool expect_named(bool condition, const char* name, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << name << ": " << message << std::endl;
    return false;
}

} // namespace

int main() {
    bool ok = true;

    namespace errors = exv::core::tunnel::errors;
    using errors::CoreErrorMapper;
    using errors::ErrorInfo;

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

    // --- from_helper_error: permission_denied is not recoverable ---
    {
        ErrorInfo err = CoreErrorMapper::from_helper_error(
            "permission_denied", "helper rejected the client identity");
        ok = expect(err.domain == "helper",
                    "helper error domain should be helper") && ok;
        ok = expect(err.code == "permission_denied",
                    "error code should match") && ok;
        ok = expect(!err.recoverable,
                    "permission_denied should not be recoverable") && ok;
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

    // --- from_native_error: preserves AnyConnect v2 protocol codes ---
    {
        struct Case {
            const char* code;
            const char* domain;
            bool recoverable;
        };
        const Case cases[] = {
            {"auth_protocol_mismatch", "auth", false},
            {"auth_rejected", "auth", false},
            {"auth_challenge_required", "auth", true},
            {"auth_group_required", "auth", true},
            {"auth_expired", "auth", true},
            {"csd_required_unsupported", "auth", false},
            {"dtls_unavailable", "transport", true},
            {"tunnel_disconnected", "transport", true},
            {"session_timeout", "transport", false},
            {"idle_timeout", "transport", true},
            {"rekey_unsupported", "transport", true},
            {"cstp_compressed_unsupported", "transport", false},
            {"unsupported_extra_args", "config", true},
        };

        for (const Case& c : cases) {
            ErrorInfo err = CoreErrorMapper::from_native_error(
                c.code, "native protocol failure");
            ok = expect(err.code == c.code,
                        "native protocol code should be preserved") && ok;
            ok = expect(err.domain == c.domain,
                        "native protocol domain should be classified") && ok;
            ok = expect_named(err.recoverable == c.recoverable, c.code,
                              "native protocol recoverability should be stable") && ok;
            ok = expect(!err.recommended_action.empty(),
                        "native protocol action should be populated") && ok;
        }
    }

    // --- ErrorInfo fields are properly set ---
    {
        ErrorInfo err = CoreErrorMapper::from_transport_error(0, "send");
        ok = expect(!err.message.empty(),
                    "error message should not be empty") && ok;
        ok = expect(!err.recommended_action.empty(),
                    "recommended_action should not be empty") && ok;
    }

    // --- native engine errors preserve stable public codes and redact secrets ---
    {
        ErrorInfo err = CoreErrorMapper::from_native_error(
            "saml_required_unsupported",
            "SSO redirect https://idp.example.invalid/login?SAMLRequest=SECRET_TOKEN");
        ok = expect(err.domain == "auth",
                    "SAML native error domain should be auth") && ok;
        ok = expect(err.code == "saml_required_unsupported",
                    "SAML native error code should be preserved") && ok;
        ok = expect(err.message.find("idp.example.invalid") == std::string::npos,
                    "SAML native error should redact URL host") && ok;
        ok = expect(err.message.find("SECRET_TOKEN") == std::string::npos,
                    "SAML native error should redact SAML request") && ok;
        ok = expect(!err.recoverable,
                    "SAML unsupported browser flow should not be recoverable") && ok;
        ok = expect(!err.recommended_action.empty(),
                    "SAML native error should include remediation") && ok;
    }

    {
        ErrorInfo err = CoreErrorMapper::from_native_error(
            "dns_resolution_failed", "failed to resolve vpn.example.invalid");
        ok = expect(err.domain == "transport",
                    "DNS native error domain should be transport") && ok;
        ok = expect(err.code == "network_unreachable",
                    "DNS native error should map to public network code") && ok;
        ok = expect(err.recoverable,
                    "DNS native error should be recoverable") && ok;
    }

    {
        ErrorInfo err = CoreErrorMapper::from_native_error(
            "dtls_fell_back_to_tls",
            "native DTLS backend unavailable; using CSTP/TLS");
        ok = expect(err.domain == "transport",
                    "DTLS fallback domain should be transport") && ok;
        ok = expect(err.code == "dtls_fell_back_to_tls",
                    "DTLS fallback code should be stable") && ok;
        ok = expect(err.recoverable,
                    "DTLS fallback should be informational/recoverable") && ok;
    }

    // --- aggregate-auth response framing errors collapse to protocol mismatch ---
    // Raw aggregate-auth codes signal a malformed gateway response, not a
    // password failure. They must leave the mapper as auth_protocol_mismatch
    // so the renderer shows "查看日志" rather than "请重新输入密码".
    {
        ErrorInfo err = CoreErrorMapper::from_native_error(
            "auth_response_invalid", "aggregate auth response is empty");
        ok = expect(err.code == "auth_protocol_mismatch",
                    "auth_response_invalid should map to auth_protocol_mismatch") && ok;
        ok = expect(err.domain == "auth",
                    "auth_response_invalid should stay in auth domain") && ok;
        ok = expect(!err.recoverable,
                    "auth_response_invalid should not be recoverable") && ok;
        ok = expect(!err.recommended_action.empty(),
                    "auth_response_invalid should carry a recommended action") && ok;
    }

    {
        ErrorInfo err = CoreErrorMapper::from_native_error(
            "auth_response_too_large",
            "aggregate auth response exceeds maximum size");
        ok = expect(err.code == "auth_protocol_mismatch",
                    "auth_response_too_large should map to auth_protocol_mismatch") && ok;
        ok = expect(err.domain == "auth",
                    "auth_response_too_large should stay in auth domain") && ok;
        ok = expect(!err.recoverable,
                    "auth_response_too_large should not be recoverable") && ok;
    }

    {
        ErrorInfo err = CoreErrorMapper::from_native_error(
            "protocol_error", "bad XML token SECRET_COOKIE webvpn=SECRET");
        ok = expect(err.domain == "protocol",
                    "protocol parser native error domain should be protocol") && ok;
        ok = expect(err.code == "protocol_error",
                    "protocol parser native error code should be stable") && ok;
        ok = expect(err.message.find("SECRET_COOKIE") == std::string::npos,
                    "protocol parser error should redact secret token") && ok;
        ok = expect(err.message.find("webvpn=SECRET") == std::string::npos,
                    "protocol parser error should redact cookie text") && ok;
    }

    if (ok) {
        std::cout << "core_error_mapper_test: all assertions passed\n";
    } else {
        std::cerr << "core_error_mapper_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
