#include "core_api_setup.hpp"
#include "vpn_actions.hpp"
#include "config_actions.hpp"
#include "service_actions.hpp"
#include "route_actions.hpp"

namespace exv::core_api {

std::unique_ptr<AppRpcDispatcher> create_dispatcher(
    std::shared_ptr<exv::core::TunnelController> controller
) {
    auto dispatcher = std::make_unique<AppRpcDispatcher>();

    auto vpn = std::make_unique<VpnActions>(controller);
    vpn->register_handlers(*dispatcher);

    ConfigActions config;
    config.register_handlers(*dispatcher);

    ServiceActions service;
    service.register_handlers(*dispatcher);

    RouteActions route;
    route.register_handlers(*dispatcher);

    return dispatcher;
}

} // namespace exv::core_api
