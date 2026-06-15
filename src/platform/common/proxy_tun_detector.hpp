#pragma once

#include "platform/common/virtual_network_model.hpp"

#include <string>

namespace ecnuvpn {
namespace platform {

bool is_proxy_tun_candidate(const std::string &name,
                            const std::string &detail,
                            const std::string &route_reason,
                            const std::string &exv_interface);

virtual_network::AdapterInfo make_proxy_tun_adapter(
    const std::string &name, const std::string &detail,
    const std::string &if_index, const std::string &route_reason);

} // namespace platform
} // namespace ecnuvpn
