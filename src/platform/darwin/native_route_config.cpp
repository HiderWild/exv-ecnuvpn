#include "platform/darwin/native_route_config.hpp"

#include <algorithm>
#include <cerrno>
#include <cstdio>
#include <cstdint>
#include <cstring>
#include <limits>
#include <set>
#include <sstream>
#include <utility>
#include <vector>

#if defined(__APPLE__)
#include <arpa/inet.h>
#include <net/if.h>
#include <net/if_dl.h>
#include <net/route.h>
#include <netinet/in.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <unistd.h>
#endif

namespace ecnuvpn {
namespace platform {
namespace {

constexpr int kNoError = 0;
constexpr int kMinimumUsableMtu = 1200;
constexpr int kMaximumMtu = 1500;

#include "platform/darwin/native_route_config_model.inc.cpp"
#include "platform/darwin/native_route_config_socket.inc.cpp"

} // namespace

#include "platform/darwin/native_route_config_public.inc.cpp"

} // namespace platform
} // namespace ecnuvpn
