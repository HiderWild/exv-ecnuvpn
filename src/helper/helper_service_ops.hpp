#pragma once

#include "helper/common/helper_messages.hpp"

#include <memory>

namespace exv::helper {

class HelperServiceOps {
public:
    virtual ~HelperServiceOps() = default;

    virtual InstallServiceResponse install_service(
        const InstallServiceRequest& request) = 0;
    virtual UninstallServiceResponse uninstall_service(
        const UninstallServiceRequest& request) = 0;
    virtual RepairServiceResponse repair_service(
        const RepairServiceRequest& request) = 0;
};

std::shared_ptr<HelperServiceOps> create_helper_service_ops();

} // namespace exv::helper
