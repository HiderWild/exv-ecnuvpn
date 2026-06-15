#include "core/tunnel_controller/tunnel_controller_impl.hpp"

#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"

namespace exv::core {

void log_tunnel_event(const std::string& level,
                      const std::string& code,
                      const std::string& message,
                      const std::vector<std::pair<std::string, std::string>>& fields) {
    exv::observability::LogFacade::event(level, "tunnel", code, message, fields);
}

std::shared_ptr<exv::platform::HelperDelegatingPlatformNetworkOps>
as_helper_delegating_ops(const std::shared_ptr<exv::platform::PlatformNetworkOps>& ops) {
    return std::dynamic_pointer_cast<exv::platform::HelperDelegatingPlatformNetworkOps>(ops);
}

} // namespace exv::core
