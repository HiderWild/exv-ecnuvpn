#pragma once

#include <functional>

namespace exv::app_api::testing {

using DesktopVpnConnectEnteredHook = std::function<void()>;

void set_desktop_vpn_connect_entered_hook(DesktopVpnConnectEnteredHook hook);
void fire_desktop_vpn_connect_entered_hook();

} // namespace exv::app_api::testing
