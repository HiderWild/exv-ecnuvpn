#include "core/vpn/vpn_legacy_adapter.hpp"
#include "core/vpn/vpn.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"

namespace ecnuvpn {
namespace vpn {
namespace legacy {

int start(const Config &cfg, const std::string &plaintext_password,
          int retry_limit) {
  // Delegate to vpn::start. The password is resolved from config internally.
  (void)plaintext_password;
  return vpn::start(cfg, retry_limit);
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
