#include "native_startup_failure.hpp"

namespace exv::core {

StartupFailureInfo NativeStartupFailureAnalyzer::classify(
    FailurePhase phase,
    const ErrorInfo& error,
    bool was_stable_ready
) {
    StartupFailureInfo info;
    info.phase = phase;
    info.error = error;

    // Terminal failures (never retry)
    if (is_terminal(error)) {
        info.is_terminal = true;
        info.is_recoverable = false;
        info.detail = "Terminal failure: " + error.code;
        return info;
    }

    // Pre-stable failures are not recoverable
    if (!was_stable_ready && phase != FailurePhase::StableReady) {
        info.is_terminal = false;
        info.is_recoverable = false;
        info.detail = "Pre-stable failure: " + error.code;
        return info;
    }

    // Post-stable transport failures are recoverable
    if (was_stable_ready && error.domain == "transport") {
        info.is_terminal = false;
        info.is_recoverable = true;
        info.detail = "Post-stable transport failure: " + error.code;
        return info;
    }

    // Default: not recoverable
    info.is_terminal = false;
    info.is_recoverable = false;
    info.detail = "Unhandled failure: " + error.code;
    return info;
}

bool NativeStartupFailureAnalyzer::is_terminal(const ErrorInfo& error) {
    // Auth failures are terminal
    if (error.domain == "auth") return true;
    // Certificate errors are terminal
    if (error.code == "cert_error") return true;
    // Credential errors are terminal
    if (error.code == "credential_expired") return true;
    return false;
}

bool NativeStartupFailureAnalyzer::is_recoverable(const ErrorInfo& error, bool was_stable_ready) {
    if (is_terminal(error)) return false;
    if (!was_stable_ready) return false;
    // Transport errors after stable are recoverable
    return error.domain == "transport";
}

ErrorInfo NativeStartupFailureAnalyzer::map_native_error(
    int native_code,
    const std::string& api,
    FailurePhase phase
) {
    ErrorInfo info;
    info.native_code = native_code;
    info.native_api = api;

    // Map common native errors
    if (api.find("TLS") != std::string::npos || api.find("SSL") != std::string::npos) {
        info.domain = "transport";
        info.code = "tls_error";
        info.recoverable = (phase == FailurePhase::StableReady);
        info.recommended_action = "Check certificate and server configuration";
    } else if (api.find("auth") != std::string::npos) {
        info.domain = "auth";
        info.code = "auth_failed";
        info.recoverable = false;
        info.recommended_action = "Check credentials";
    } else if (api.find("connect") != std::string::npos) {
        info.domain = "transport";
        info.code = "transport_closed";
        info.recoverable = (phase == FailurePhase::StableReady);
        info.recommended_action = "Check network connectivity";
    } else {
        info.domain = "unknown";
        info.code = "native_error";
        info.recoverable = false;
        info.recommended_action = "Check logs for details";
    }

    info.message = "Native error in " + api + ": code " + std::to_string(native_code);
    return info;
}

} // namespace exv::core
