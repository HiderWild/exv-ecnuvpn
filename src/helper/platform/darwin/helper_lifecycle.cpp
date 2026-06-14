#include "helper/platform/helper_lifecycle.hpp"

#include "helper/helper_ipc.hpp"
#include "logger.hpp"
#include "helper/platform/helper_platform.hpp"
#include "tunnel.hpp"
#include "utils.hpp"

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

#include "helper/platform/darwin/helper_lifecycle_version.inc.cpp"
#include "helper/platform/darwin/helper_lifecycle_install.inc.cpp"
#include "helper/platform/darwin/helper_lifecycle_worker.inc.cpp"

} // namespace platform
} // namespace ecnuvpn
