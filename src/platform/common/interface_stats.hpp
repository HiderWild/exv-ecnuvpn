#pragma once

#include <cstdint>
#include <string>

namespace exv::platform {

bool get_interface_traffic(const std::string &iface, std::uint64_t *rx_bytes,
                           std::uint64_t *tx_bytes);

} // namespace exv::platform

