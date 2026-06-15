// Tests for ErrorInfo serialization: to_json/from_json round-trip,
// field presence, optional field handling, error code constants.

#include "feedback/error_contract.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <string>

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char* message) {
    if (condition) return true;
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

} // namespace

int main() {
    bool ok = true;

    using exv::feedback::ErrorInfo;

    // --- to_json/from_json round-trip with all fields ---
    {
        ErrorInfo original;
        original.domain = "transport";
        original.code = "transport_closed";
        original.message = "TLS connection dropped by server";
        original.native_code = 10054;
        original.native_api = "WSARecv";
        original.recoverable = true;
        original.recommended_action = "retry";

        std::string serialized = original.to_json();
        ErrorInfo restored = ErrorInfo::from_json(serialized);

        ok = expect(restored.domain == "transport",
                    "domain should survive round-trip") && ok;
        ok = expect(restored.code == "transport_closed",
                    "code should survive round-trip") && ok;
        ok = expect(restored.message == "TLS connection dropped by server",
                    "message should survive round-trip") && ok;
        ok = expect(restored.native_code.has_value() && *restored.native_code == 10054,
                    "native_code should survive round-trip") && ok;
        ok = expect(restored.native_api == "WSARecv",
                    "native_api should survive round-trip") && ok;
        ok = expect(restored.recoverable == true,
                    "recoverable should survive round-trip") && ok;
        ok = expect(restored.recommended_action == "retry",
                    "recommended_action should survive round-trip") && ok;
    }

    // --- to_json output contains all required fields ---
    {
        ErrorInfo info;
        info.domain = "auth";
        info.code = "auth_failed";
        info.message = "Bad credentials";
        info.recoverable = false;
        info.recommended_action = "report";

        auto j = json::parse(info.to_json());

        ok = expect(j.contains("domain"), "JSON missing 'domain'") && ok;
        ok = expect(j.contains("code"), "JSON missing 'code'") && ok;
        ok = expect(j.contains("message"), "JSON missing 'message'") && ok;
        ok = expect(j.contains("recoverable"), "JSON missing 'recoverable'") && ok;
        ok = expect(j.contains("recommended_action"), "JSON missing 'recommended_action'") && ok;
        ok = expect(j.contains("native_api"), "JSON missing 'native_api'") && ok;
    }

    // --- optional native_code is omitted when not set ---
    {
        ErrorInfo info;
        info.domain = "helper";
        info.code = "helper_unavailable";
        info.message = "Cannot connect to helper";
        info.recoverable = false;
        info.recommended_action = "reconnect_helper";

        auto j = json::parse(info.to_json());

        // native_code is optional — should not be present when not set
        ok = expect(!j.contains("native_code") || j["native_code"].is_null(),
                    "native_code should be absent or null when not set") && ok;
    }

    // --- from_json handles missing optional fields gracefully ---
    {
        std::string minimal_json = R"({
            "domain": "packet",
            "code": "device_open_failed",
            "message": "Cannot open TUN device",
            "recoverable": false,
            "recommended_action": "report"
        })";

        ErrorInfo info = ErrorInfo::from_json(minimal_json);

        ok = expect(info.domain == "packet",
                    "domain should parse correctly") && ok;
        ok = expect(info.code == "device_open_failed",
                    "code should parse correctly") && ok;
        ok = expect(info.recoverable == false,
                    "recoverable should parse correctly") && ok;
        ok = expect(!info.native_code.has_value(),
                    "native_code should be empty when not in JSON") && ok;
        ok = expect(info.native_api.empty(),
                    "native_api should be empty when not in JSON") && ok;
    }

    // --- from_json handles null native_code ---
    {
        std::string json_with_null = R"({
            "domain": "transport",
            "code": "transport_closed",
            "message": "Connection reset",
            "native_code": null,
            "native_api": "",
            "recoverable": true,
            "recommended_action": "retry"
        })";

        ErrorInfo info = ErrorInfo::from_json(json_with_null);

        ok = expect(!info.native_code.has_value(),
                    "null native_code should result in empty optional") && ok;
    }

    // --- error_domains constants are correct strings ---
    {
        using namespace exv::feedback::error_domains;
        ok = expect(std::string(TRANSPORT) == "transport",
                    "TRANSPORT domain should be 'transport'") && ok;
        ok = expect(std::string(AUTH) == "auth",
                    "AUTH domain should be 'auth'") && ok;
        ok = expect(std::string(HELPER) == "helper",
                    "HELPER domain should be 'helper'") && ok;
        ok = expect(std::string(OS_ROUTE) == "os.route",
                    "OS_ROUTE domain should be 'os.route'") && ok;
        ok = expect(std::string(OS_DNS) == "os.dns",
                    "OS_DNS domain should be 'os.dns'") && ok;
        ok = expect(std::string(PACKET) == "packet",
                    "PACKET domain should be 'packet'") && ok;
        ok = expect(std::string(CONFIG) == "config",
                    "CONFIG domain should be 'config'") && ok;
    }

    // --- error_codes constants are correct strings ---
    {
        using namespace exv::feedback::error_codes;
        ok = expect(std::string(TRANSPORT_CLOSED) == "transport_closed",
                    "TRANSPORT_CLOSED should be 'transport_closed'") && ok;
        ok = expect(std::string(TRANSPORT_TIMEOUT) == "transport_timeout",
                    "TRANSPORT_TIMEOUT should be 'transport_timeout'") && ok;
        ok = expect(std::string(TLS_ERROR) == "tls_error",
                    "TLS_ERROR should be 'tls_error'") && ok;
        ok = expect(std::string(AUTH_FAILED) == "auth_failed",
                    "AUTH_FAILED should be 'auth_failed'") && ok;
        ok = expect(std::string(CERT_ERROR) == "cert_error",
                    "CERT_ERROR should be 'cert_error'") && ok;
        ok = expect(std::string(CREDENTIAL_EXPIRED) == "credential_expired",
                    "CREDENTIAL_EXPIRED should be 'credential_expired'") && ok;
        ok = expect(std::string(HELPER_UNAVAILABLE) == "helper_unavailable",
                    "HELPER_UNAVAILABLE should be 'helper_unavailable'") && ok;
        ok = expect(std::string(HELPER_PROTOCOL_REJECTED) == "helper_protocol_rejected",
                    "HELPER_PROTOCOL_REJECTED should be 'helper_protocol_rejected'") && ok;
        ok = expect(std::string(HELPER_TIMEOUT) == "helper_timeout",
                    "HELPER_TIMEOUT should be 'helper_timeout'") && ok;
        ok = expect(std::string(ROUTE_FAILED) == "route_failed",
                    "ROUTE_FAILED should be 'route_failed'") && ok;
        ok = expect(std::string(DNS_FAILED) == "dns_failed",
                    "DNS_FAILED should be 'dns_failed'") && ok;
        ok = expect(std::string(DEVICE_FAILED) == "device_failed",
                    "DEVICE_FAILED should be 'device_failed'") && ok;
        ok = expect(std::string(FIREWALL_FAILED) == "firewall_failed",
                    "FIREWALL_FAILED should be 'firewall_failed'") && ok;
        ok = expect(std::string(INVALID_CONFIG) == "invalid_config",
                    "INVALID_CONFIG should be 'invalid_config'") && ok;
        ok = expect(std::string(PROFILE_NOT_FOUND) == "profile_not_found",
                    "PROFILE_NOT_FOUND should be 'profile_not_found'") && ok;
    }

    // --- round-trip preserves all error domain values ---
    {
        const char* domains[] = {
            "transport", "auth", "helper", "os.route", "os.dns", "packet", "config"
        };
        for (const char* d : domains) {
            ErrorInfo info;
            info.domain = d;
            info.code = "test_code";
            info.message = "test";
            info.recoverable = true;
            info.recommended_action = "retry";

            auto restored = ErrorInfo::from_json(info.to_json());
            ok = expect(restored.domain == d,
                        ("domain round-trip for: " + std::string(d)).c_str()) && ok;
        }
    }

    if (ok) {
        std::cout << "error_contract_test: all assertions passed\n";
    } else {
        std::cerr << "error_contract_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
