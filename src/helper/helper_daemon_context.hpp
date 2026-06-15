#pragma once

#include "helper/helper.hpp"
#include "helper/helper_handler.hpp"
#include "helper/helper_network_ops.hpp"
#include "helper/helper_service_ops.hpp"

#include <functional>
#include <memory>

namespace ecnuvpn::helper {

using HelperNetworkOpsFactory =
    std::function<std::shared_ptr<exv::helper::HelperNetworkOps>()>;
using HelperServiceOpsFactory =
    std::function<std::shared_ptr<exv::helper::HelperServiceOps>()>;

std::unique_ptr<exv::helper::HelperHandler> create_helper_handler_for_daemon(
    const DaemonOptions &options,
    HelperNetworkOpsFactory network_ops_factory = HelperNetworkOpsFactory{},
    HelperServiceOpsFactory service_ops_factory = HelperServiceOpsFactory{});

} // namespace ecnuvpn::helper
