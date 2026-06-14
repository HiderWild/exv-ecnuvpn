#include "platform/common/oneshot_bootstrap.hpp"

namespace ecnuvpn {
namespace platform {

nlohmann::json oneshot_backend_to_json(const OneshotBackend &backend) {
  nlohmann::json result{{"ok", backend.ok},
                        {"backend", "oneshot"},
                        {"mode", backend.mode},
                        {"transport", backend.transport},
                        {"endpoint", backend.endpoint},
                        {"owner", backend.owner},
                        {"parent_pid", backend.parent_pid},
                        {"pid", backend.pid}};
  if (!backend.code.empty())
    result["code"] = backend.code;
  if (!backend.message.empty())
    result["message"] = backend.message;
  return result;
}

} // namespace platform
} // namespace ecnuvpn
