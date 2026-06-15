#include "helper/helper_service_ops.hpp"

#include "helper/common/helper_messages.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/helper_lifecycle.hpp"
#include "platform/common/helper_service_manager.hpp"
#include "platform/common/process_utils.hpp"

#include <memory>
#include <string>

namespace exv::helper {
namespace {

bool send_request_for_platform(const nlohmann::json& request,
                               nlohmann::json* response,
                               std::string* error_message,
                               int /*timeout_seconds*/) {
    nlohmann::json result = ecnuvpn::platform::send_helper_request(request);
    if (!result.is_object()) {
        if (error_message)
            *error_message = "Failed to parse EXV helper response.";
        return false;
    }
    if (result.contains("ok") && result["ok"].is_boolean() &&
        !result["ok"].get<bool>()) {
        if (error_message) {
            *error_message =
                result.value("message", std::string("EXV helper request failed."));
        }
        return false;
    }
    if (response)
        *response = result;
    return true;
}

bool wait_until_available_for_platform(int attempts, unsigned int delay_us) {
    for (int i = 0; i < attempts; ++i) {
        HelloRequest hello_req;
        HelperRequest request;
        request.op = HelperOp::Hello;
        request.payload_json = nlohmann::json(hello_req).dump();

        nlohmann::json response;
        std::string error_message;
        if (send_request_for_platform(nlohmann::json(request), &response,
                                      &error_message, 15)) {
            return true;
        }
        if (i + 1 < attempts && delay_us > 0) {
            ecnuvpn::platform::sleep_ms(static_cast<int>(delay_us / 1000));
        }
    }
    return false;
}

ecnuvpn::platform::HelperServiceManagerContext service_manager_context() {
    return ecnuvpn::platform::HelperServiceManagerContext{
        wait_until_available_for_platform,
        send_request_for_platform,
        nullptr,
        nullptr,
    };
}

class PlatformBackedHelperServiceOps final : public HelperServiceOps {
public:
    InstallServiceResponse install_service(
        const InstallServiceRequest& request) override {
        (void)request;
        const int exit_code = ecnuvpn::platform::install_helper_service(
            std::string{}, service_manager_context());
        InstallServiceResponse response;
        response.success = exit_code == 0;
        response.exit_code = exit_code;
        response.message =
            response.success ? "Helper service installed"
                             : "Helper service installation failed";
        return response;
    }

    UninstallServiceResponse uninstall_service(
        const UninstallServiceRequest& request) override {
        (void)request;
        const int exit_code = ecnuvpn::platform::uninstall_helper_service(
            service_manager_context());
        UninstallServiceResponse response;
        response.success = exit_code == 0;
        response.exit_code = exit_code;
        response.message =
            response.success ? "Helper service uninstalled"
                             : "Helper service uninstallation failed";
        return response;
    }
};

} // namespace

std::shared_ptr<HelperServiceOps> create_helper_service_ops() {
    return std::make_shared<PlatformBackedHelperServiceOps>();
}

} // namespace exv::helper
