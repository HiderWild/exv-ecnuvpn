#include "platform/common/oneshot_bootstrap.hpp"

#include "platform/common/backend_resolver.hpp"

namespace ecnuvpn {
namespace platform {

OneshotBackend start_oneshot_helper(const OneshotBootstrapRequest &request) {
  (void)request;
  OneshotBackend backend;
  backend.transport = "unix-socket";
  backend.code = kOneshotNotSupportedCode;
  backend.message = "One-shot helper mode is not implemented on Linux.";
  return backend;
}

} // namespace platform
} // namespace ecnuvpn
