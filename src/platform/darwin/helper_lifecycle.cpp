#include "platform/common/helper_lifecycle.hpp"

#include "helper/helper_ipc.hpp"
#include "logger.hpp"
#include "platform/common/helper_platform.hpp"
#include "cli/console.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_paths.hpp"
#include "tunnel.hpp"
#include "utils/strings.hpp"

#include <cctype>
#include <cerrno>
#include <csignal>
#include <cstdio>
#include <cstring>
#include <fcntl.h>
#include <filesystem>
#include <iostream>
#include <sstream>
#include <string>
#include <sys/stat.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

namespace ecnuvpn {
namespace platform {
namespace {

struct SemanticVersion {
  int major = -1;
  int minor = -1;
  int patch = -1;
};

#include "platform/darwin/helper_lifecycle_version.inc.cpp"
#include "platform/darwin/helper_lifecycle_install.inc.cpp"
#include "platform/darwin/helper_lifecycle_worker.inc.cpp"

} // namespace platform
} // namespace ecnuvpn
