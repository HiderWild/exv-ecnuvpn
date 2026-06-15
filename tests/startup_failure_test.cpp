// Tests for NativeStartupFailureAnalyzer: verifies failure classification,
// terminal/recoverable determination, and native error mapping.

#include "core/tunnel_controller/native_startup_failure.hpp"
#include "core/tunnel_controller/tunnel_state.hpp"

#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

exv::core::ErrorInfo make_error(const std::string& domain, const std::string& code,
                                bool recoverable = false) {
    exv::core::ErrorInfo e;
    e.domain = domain;
    e.code = code;
    e.message = "test: " + code;
    e.recoverable = recoverable;
    return e;
}

} // namespace

int main() {
    bool ok = true;

    using exv::core::ErrorInfo;
    using exv::core::FailurePhase;
    using exv::core::NativeStartupFailureAnalyzer;
    using exv::core::StartupFailureInfo;

    // --- Auth failure is terminal regardless of phase ---
    {
        auto error = make_error("auth", "auth_failed");
        auto info = NativeStartupFailureAnalyzer::classify(
            FailurePhase::PreConnect, error, false);
        ok = expect(info.is_terminal,
                    "auth failure should be terminal (PreConnect)") && ok;
        ok = expect(!info.is_recoverable,
                    "auth failure should not be recoverable") && ok;
    }
    {
        auto error = make_error("auth", "auth_failed");
        auto info = NativeStartupFailureAnalyzer::classify(
            FailurePhase::Authenticating, error, false);
        ok = expect(info.is_terminal,
                    "auth failure should be terminal (Authenticating)") && ok;
    }
    {
        auto error = make_error("auth", "auth_failed");
        auto info = NativeStartupFailureAnalyzer::classify(
            FailurePhase::StableReady, error, true);
        ok = expect(info.is_terminal,
                    "auth failure should be terminal even when stable") && ok;
    }

    // --- Cert error is terminal ---
    {
        auto error = make_error("transport", "cert_error");
        ok = expect(NativeStartupFailureAnalyzer::is_terminal(error),
                    "cert_error should be terminal") && ok;
    }

    // --- Credential expired is terminal ---
    {
        auto error = make_error("auth", "credential_expired");
        ok = expect(NativeStartupFailureAnalyzer::is_terminal(error),
                    "credential_expired should be terminal") && ok;
    }

    // --- Pre-stable transport failure is not recoverable ---
    {
        auto error = make_error("transport", "transport_closed", true);
        auto info = NativeStartupFailureAnalyzer::classify(
            FailurePhase::Connecting, error, false);
        ok = expect(!info.is_terminal,
                    "pre-stable transport failure should not be terminal") && ok;
        ok = expect(!info.is_recoverable,
                    "pre-stable transport failure should not be recoverable") && ok;
        ok = expect(info.phase == FailurePhase::Connecting,
                    "phase should be preserved") && ok;
        ok = expect(info.detail.find("Pre-stable") != std::string::npos,
                    "detail should mention pre-stable") && ok;
    }

    // --- Post-stable transport failure is recoverable ---
    {
        auto error = make_error("transport", "transport_closed", true);
        auto info = NativeStartupFailureAnalyzer::classify(
            FailurePhase::StableReady, error, true);
        ok = expect(!info.is_terminal,
                    "post-stable transport failure should not be terminal") && ok;
        ok = expect(info.is_recoverable,
                    "post-stable transport failure should be recoverable") && ok;
        ok = expect(info.detail.find("Post-stable") != std::string::npos,
                    "detail should mention post-stable") && ok;
    }

    // --- Post-stable non-transport failure is not recoverable ---
    {
        auto error = make_error("os.route", "route_failed", false);
        auto info = NativeStartupFailureAnalyzer::classify(
            FailurePhase::StableReady, error, true);
        ok = expect(!info.is_terminal,
                    "post-stable route failure should not be terminal") && ok;
        ok = expect(!info.is_recoverable,
                    "post-stable route failure should not be recoverable") && ok;
    }

    // --- Unhandled failure defaults (was_stable but non-transport domain) ---
    {
        auto error = make_error("unknown", "something_weird", false);
        auto info = NativeStartupFailureAnalyzer::classify(
            FailurePhase::StableReady, error, true);
        ok = expect(!info.is_terminal,
                    "unknown failure should not be terminal") && ok;
        ok = expect(!info.is_recoverable,
                    "unknown failure should not be recoverable") && ok;
        ok = expect(info.detail.find("Unhandled") != std::string::npos,
                    "detail should mention unhandled") && ok;
    }

    // --- map_native_error: TLS/SSL errors ---
    {
        auto err = NativeStartupFailureAnalyzer::map_native_error(
            -1, "TLS_handshake", FailurePhase::Connecting);
        ok = expect(err.domain == "transport",
                    "TLS error should map to transport domain") && ok;
        ok = expect(err.code == "tls_error",
                    "TLS error code should be tls_error") && ok;
        ok = expect(!err.recoverable,
                    "TLS error during Connecting should not be recoverable") && ok;
        ok = expect(err.native_code.has_value() && *err.native_code == -1,
                    "native_code should be preserved") && ok;
        ok = expect(err.native_api == "TLS_handshake",
                    "native_api should be preserved") && ok;
    }
    {
        auto err = NativeStartupFailureAnalyzer::map_native_error(
            -1, "SSL_read", FailurePhase::StableReady);
        ok = expect(err.recoverable,
                    "SSL error during StableReady should be recoverable") && ok;
    }

    // --- map_native_error: auth errors ---
    {
        auto err = NativeStartupFailureAnalyzer::map_native_error(
            401, "auth_login", FailurePhase::Authenticating);
        ok = expect(err.domain == "auth",
                    "auth API should map to auth domain") && ok;
        ok = expect(err.code == "auth_failed",
                    "auth API code should be auth_failed") && ok;
        ok = expect(!err.recoverable,
                    "auth errors should not be recoverable") && ok;
    }

    // --- map_native_error: connect errors ---
    {
        auto err = NativeStartupFailureAnalyzer::map_native_error(
            10054, "connect_socket", FailurePhase::Connecting);
        ok = expect(err.domain == "transport",
                    "connect API should map to transport domain") && ok;
        ok = expect(err.code == "transport_closed",
                    "connect API code should be transport_closed") && ok;
        ok = expect(!err.recoverable,
                    "connect error during Connecting should not be recoverable") && ok;
    }
    {
        auto err = NativeStartupFailureAnalyzer::map_native_error(
            10054, "connect_socket", FailurePhase::StableReady);
        ok = expect(err.recoverable,
                    "connect error during StableReady should be recoverable") && ok;
    }

    // --- map_native_error: unknown API ---
    {
        auto err = NativeStartupFailureAnalyzer::map_native_error(
            42, "some_other_api", FailurePhase::PreConnect);
        ok = expect(err.domain == "unknown",
                    "unknown API should map to unknown domain") && ok;
        ok = expect(err.code == "native_error",
                    "unknown API code should be native_error") && ok;
        ok = expect(!err.recoverable,
                    "unknown errors should not be recoverable") && ok;
    }

    // --- classify preserves error info ---
    {
        auto error = make_error("transport", "transport_closed");
        error.native_code = 10054;
        error.native_api = "recv";
        auto info = NativeStartupFailureAnalyzer::classify(
            FailurePhase::Connecting, error, false);
        ok = expect(info.error.native_code.has_value() &&
                    *info.error.native_code == 10054,
                    "classify should preserve native_code in error info") && ok;
        ok = expect(info.error.native_api == "recv",
                    "classify should preserve native_api in error info") && ok;
    }

    if (ok) {
        std::cout << "startup_failure_test: all assertions passed\n";
    } else {
        std::cerr << "startup_failure_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
