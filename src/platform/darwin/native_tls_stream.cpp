#include "platform/darwin/native_tls_stream.hpp"

#include <Security/SecureTransport.h>
#include <Security/Security.h>
#include <CoreFoundation/CoreFoundation.h>

#include <arpa/inet.h>
#include <errno.h>
#include <fcntl.h>
#include <netdb.h>
#include <sys/select.h>
#include <sys/socket.h>
#include <unistd.h>

#include <algorithm>
#include <cstddef>
#include <cstring>
#include <limits>
#include <sstream>
#include <string>
#include <utility>

namespace ecnuvpn {
namespace platform {

namespace {

constexpr int kConnectTimeoutMs = 15000;
constexpr std::size_t kPlaintextReadSize = 16 * 1024;
constexpr int kMaxHandshakeSteps = 128;

#include "platform/darwin/native_tls_stream_errors.inc.cpp"
#include "platform/darwin/native_tls_stream_cf_socket.inc.cpp"
#include "platform/darwin/native_tls_stream_context.inc.cpp"
#include "platform/darwin/native_tls_stream_api.inc.cpp"

} // namespace

#include "platform/darwin/native_tls_stream_deps.inc.cpp"
#include "platform/darwin/native_tls_stream_public.inc.cpp"

} // namespace platform
} // namespace ecnuvpn
