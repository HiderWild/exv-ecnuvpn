#pragma once

namespace exv::core_api {
class DesktopRpcAdapter;
}

namespace ecnuvpn {
namespace app_api {

void register_desktop_vpn_actions(exv::core_api::DesktopRpcAdapter &adapter);
void shutdown_desktop_vpn_runtime();

} // namespace app_api
} // namespace ecnuvpn
