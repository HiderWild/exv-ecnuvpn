#include "platform/common/tunnel_script.hpp"

#include "platform/common/process_utils.hpp"

#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>

#include "logger.hpp"
#include "openconnect_log.hpp"

#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <cstdio>
#include <fstream>
#include <regex>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

namespace ecnuvpn {
namespace platform {
namespace {

#include "platform/win32/tunnel_script_timing.inc.cpp"
#include "platform/win32/tunnel_script_helpers.inc.cpp"
#include "platform/win32/tunnel_script_configure.inc.cpp"
#include "platform/win32/tunnel_script_generator.inc.cpp"
#include "platform/win32/tunnel_script_runtime.inc.cpp"

} // namespace platform
} // namespace ecnuvpn
