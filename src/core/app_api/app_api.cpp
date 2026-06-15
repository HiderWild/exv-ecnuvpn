#include "core/app_api/app_api.hpp"

#include "core/config/config.hpp"
#include "core/config/config_api.hpp"
#include "core/config/config_manager.hpp"
#include "connection_attempt.hpp"
#include "core/tunnel_controller/timing.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "crypto.hpp"
#include "feedback/feedback.hpp"
#include "helper/helper.hpp"
#include "logger.hpp"
#include "platform/common/app_api_runtime_policy.hpp"
#include "platform/common/backend_resolver.hpp"
#include "platform/common/helper_client.hpp"
#include "platform/common/driver_status.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/oneshot_bootstrap.hpp"
#include "platform/common/path_utils.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/process_control.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/runtime_status.hpp"
#include "platform/common/service_status.hpp"
#include "runtime/runtime_context.hpp"
#include "core/tunnel_controller/tunnel_controller_active.hpp"
#include "virtual_network.hpp"
#include "vpn_engine/native_engine.hpp"
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "helper/common/helper_connector.hpp"
#include "platform/common/helper_delegating_network_ops.hpp"
#include "utils/strings.hpp"

#include <algorithm>
#include <cstdio>
#include <cstdint>
#include <chrono>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <string>
#include <utility>
#include <vector>
#ifndef _WIN32
#include <sys/stat.h>
#endif

namespace ecnuvpn {
namespace app_api {
namespace {

#include "core/app_api/app_api_json_helpers.inc.cpp"
#include "core/app_api/app_api_controller_helpers.inc.cpp"

} // namespace

#include "core/app_api/app_api_desktop_handlers.inc.cpp"

} // namespace app_api
} // namespace ecnuvpn
