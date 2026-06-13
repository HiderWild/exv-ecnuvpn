#include "utils.hpp"

#include "platform/common/path_utils.hpp"

#include <array>
#include <cstdint>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <iostream>
#ifdef __APPLE__
#include <mach-o/dyld.h>
#endif
#ifdef _WIN32
#include <windows.h>
#include <iphlpapi.h>
#include <direct.h>
#include <io.h>
#include <sys/stat.h>
#else
#include <pwd.h>
#include <sys/stat.h>
#include <unistd.h>
#endif
#include <memory>
#include <sstream>
#include <vector>

namespace ecnuvpn {
namespace utils {
#include "utils_runtime_paths.inc.cpp"
#include "utils_terminal.inc.cpp"
#include "utils_paths.inc.cpp"
#include "utils_file_io.inc.cpp"
#include "utils_runtime_discovery.inc.cpp"
#include "utils_interface_stats.inc.cpp"
#include "utils_process.inc.cpp"
#include "utils_strings.inc.cpp"

} // namespace utils
} // namespace ecnuvpn
