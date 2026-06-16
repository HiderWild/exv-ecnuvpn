#include "core_api_setup.hpp"
#include "core_actions.hpp"
#include "vpn_actions.hpp"
#include "config_actions.hpp"
#include "service_actions.hpp"
#include "route_actions.hpp"

namespace exv::core_api {

std::unique_ptr<AppRpcDispatcher> create_dispatcher(
    std::shared_ptr<exv::core::TunnelController> controller
) {
    auto dispatcher = std::make_unique<AppRpcDispatcher>();

    // All action objects are heap-allocated and retained by the dispatcher.
    // Their handlers capture `this`, so the action objects must outlive the
    // dispatcher.  retain_action() stores them as type-erased shared_ptrs.
    auto core = std::make_shared<CoreActions>();
    core->register_handlers(*dispatcher);
    dispatcher->retain_action(core);

    auto vpn = std::make_shared<VpnActions>(controller);
    vpn->register_handlers(*dispatcher);
    dispatcher->retain_action(vpn);

    auto config = std::make_shared<ConfigActions>();
    config->register_handlers(*dispatcher);
    dispatcher->retain_action(config);

    auto service = std::make_shared<ServiceActions>(controller);
    service->register_handlers(*dispatcher);
    dispatcher->retain_action(service);

    auto route = std::make_shared<RouteActions>();
    route->register_handlers(*dispatcher);
    dispatcher->retain_action(route);

    return dispatcher;
}

} // namespace exv::core_api
