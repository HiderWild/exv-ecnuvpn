#include "contracts/generated/system_contract.hpp"
#include "core/rpc/core_api_setup.hpp"
#include "core/tunnel_controller/reconnect_policy.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"

#include <nlohmann/json.hpp>

#include <iostream>
#include <algorithm>
#include <cctype>
#include <memory>
#include <string>

#ifndef ECNUVPN_VERSION
#define ECNUVPN_VERSION "test"
#endif

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char* message) {
    if (condition) {
        return true;
    }
    std::cerr << "EXPECT FAILED: " << message << std::endl;
    return false;
}

exv::core_api::RpcResponse dispatch(exv::core_api::AppRpcDispatcher& dispatcher,
                                    const std::string& action,
                                    const std::string& payload = "{}") {
    exv::core_api::RpcRequest req;
    req.action = action;
    req.payload_json = payload;
    req.request_id = "core-hello-test";
    return dispatcher.dispatch(req);
}

bool is_lower_hex(const std::string& value) {
    return !value.empty() &&
           std::all_of(value.begin(), value.end(), [](unsigned char ch) {
               return std::isdigit(ch) || (ch >= 'a' && ch <= 'f');
           });
}

} // namespace

int main() {
    bool ok = true;

    auto helper = std::shared_ptr<exv::helper::HelperClient>(nullptr);
    auto net_ops = std::shared_ptr<exv::platform::PlatformNetworkOps>(nullptr);
    auto controller = std::make_shared<exv::core::TunnelController>(
        helper, net_ops, exv::core::ReconnectConfig{});

    auto dispatcher = exv::core_api::create_dispatcher(controller);

    json first_payload;
    bool have_first_payload = false;
    auto response = dispatch(
        *dispatcher,
        "core.hello",
        R"({"contract_version":"2026-06-16.cli-core-ui-contract.v1"})");
    ok = expect(response.success,
                "core.hello should succeed for accepted contract version") && ok;
    if (response.success) {
        first_payload = json::parse(response.payload_json);
        have_first_payload = true;
        const auto& payload = first_payload;
        ok = expect(payload["ipc_protocol_version"] == "ipc-v1",
                    "hello returns protocol version string") && ok;
        ok = expect(payload["contract_version"] ==
                        std::string(exv::contracts::generated::CONTRACT_VERSION),
                    "hello returns contract version") &&
             ok;
        ok = expect(payload["app_version"] == ECNUVPN_VERSION,
                    "hello returns app version") && ok;
        ok = expect(payload.contains("core_instance_id"),
                    "hello returns instance id") && ok;
        ok = expect(payload.contains("pid"), "hello returns pid") && ok;
        ok = expect(payload.contains("core_path"),
                    "hello returns core path") && ok;
        ok = expect(payload.contains("started_at"),
                    "hello returns started_at") && ok;
        const std::string instance_id =
            payload.at("core_instance_id").get<std::string>();
        ok = expect(instance_id.rfind("core-", 0) == 0,
                    "hello instance id uses core- prefix") && ok;
        const std::string suffix = instance_id.substr(5);
        ok = expect(instance_id.find('-', 5) == std::string::npos,
                    "hello instance id uses random-success format without timestamp/pid separators") &&
             ok;
        ok = expect(suffix.size() == 16,
                    "hello instance id random-success suffix is 16 hex chars") &&
             ok;
        ok = expect(is_lower_hex(suffix),
                    "hello instance id random-success suffix is lowercase hex") &&
             ok;
    }

    auto repeated = dispatch(
        *dispatcher,
        "core.hello",
        R"({"contract_version":"2026-06-16.cli-core-ui-contract.v1"})");
    ok = expect(repeated.success,
                "repeated core.hello should succeed for accepted contract version") &&
         ok;
    if (have_first_payload && repeated.success) {
        auto repeated_payload = json::parse(repeated.payload_json);
        ok = expect(repeated_payload.value("core_instance_id", std::string()) ==
                        first_payload.value("core_instance_id", std::string()),
                    "repeated core.hello keeps core_instance_id stable") &&
             ok;
        ok = expect(repeated_payload.value("started_at", std::string()) ==
                        first_payload.value("started_at", std::string()),
                    "repeated core.hello keeps started_at stable") &&
             ok;
        ok = expect(repeated_payload.value("pid", -1) ==
                        first_payload.value("pid", -2),
                    "repeated core.hello keeps pid stable") &&
             ok;
    }

    auto bad = dispatch(*dispatcher, "core.hello", R"({"contract_version":"wrong"})");
    ok = expect(!bad.success, "core.hello rejects incompatible contract version") && ok;
    ok = expect(bad.error_code == "unsupported_contract_version",
                "mismatch uses unsupported_contract_version") &&
         ok;

    if (ok) {
        std::cout << "core_hello_actions_test: all assertions passed\n";
    } else {
        std::cerr << "core_hello_actions_test: some assertions FAILED\n";
    }
    return ok ? 0 : 1;
}
