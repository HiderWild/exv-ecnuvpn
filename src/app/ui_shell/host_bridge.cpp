#include "app/ui_shell/host_bridge.hpp"

#include "contracts/generated/system_contract.hpp"

#include <algorithm>

namespace ecnuvpn::ui_shell {

bool is_allowed_host_action(std::string_view action) {
  const auto &actions = exv::contracts::generated::DESKTOP_RPC_ACTIONS;
  return std::find(actions.begin(), actions.end(), action) != actions.end();
}

} // namespace ecnuvpn::ui_shell
