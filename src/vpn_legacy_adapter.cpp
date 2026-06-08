#include "vpn_legacy_adapter.hpp"
#include "vpn.hpp"
#include "logger.hpp"
#include "utils.hpp"

namespace ecnuvpn {
namespace vpn {
namespace legacy {

int start(const Config &cfg, const std::string &plaintext_password,
          int retry_limit) {
  // Delegate to the existing vpn::start_with_password for now.
  // The legacy path will be gradually decommissioned as the
  // TunnelController-native path takes over all connection flows.
  return vpn::start_with_password(cfg, plaintext_password, retry_limit);
}

int stop() {
  return vpn::stop();
}

int status() {
  return vpn::status();
}

} // namespace legacy
} // namespace vpn
} // namespace ecnuvpn
