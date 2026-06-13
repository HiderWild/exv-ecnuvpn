#include "platform/darwin/native_packet_device.hpp"

#include <cerrno>
#include <cstddef>
#include <cstdint>
#include <functional>
#include <string>
#include <utility>
#include <vector>

#if defined(__APPLE__)
#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <unistd.h>
#endif

namespace ecnuvpn {
namespace platform {
namespace {

constexpr std::size_t kUtunAddressFamilyHeaderSize = 4;
constexpr std::size_t kMaximumIpPacketSize = 65535;
constexpr std::size_t kMaximumUtunFrameSize =
    kUtunAddressFamilyHeaderSize + kMaximumIpPacketSize;
constexpr int kDefaultMtu = 1290;
constexpr int kUtunIoPollTimeoutMs = 100;
constexpr int kMaximumTransientWriteRetries = 16;

#include "platform/darwin/native_packet_device_helpers.inc.cpp"
#include "platform/darwin/native_packet_device_sessions.inc.cpp"
#include "platform/darwin/native_packet_device_errors.inc.cpp"
#include "platform/darwin/native_packet_device_public.inc.cpp"

} // namespace platform
} // namespace ecnuvpn
