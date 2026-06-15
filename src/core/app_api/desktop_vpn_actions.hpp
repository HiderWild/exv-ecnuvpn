#pragma once

namespace exv::core_api {
class DesktopRpcAdapter;
}

namespace ecnuvpn {
namespace app_api {

void register_desktop_vpn_actions(exv::core_api::DesktopRpcAdapter &adapter);

} // namespace app_api
} // namespace ecnuvpn
