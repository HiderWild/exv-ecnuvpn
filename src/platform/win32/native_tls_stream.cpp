#include "platform/win32/native_tls_stream.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#ifndef SECURITY_WIN32
#define SECURITY_WIN32
#endif

#include <winsock2.h>
#include <ws2tcpip.h>

#include <windows.h>
#include <schannel.h>
#include <security.h>
#include <wincrypt.h>

#include <algorithm>
#include <cstring>
#include <limits>
#include <sstream>
#include <string>
#include <utility>

namespace ecnuvpn {
namespace platform {

namespace {

constexpr int kConnectTimeoutMs = 15000;
constexpr std::size_t kEncryptedReadSize = 16 * 1024;
constexpr int kMaxHandshakeSteps = 64;

#include "platform/win32/native_tls_stream_errors.inc.cpp"
#include "platform/win32/native_tls_stream_socket.inc.cpp"
#include "platform/win32/native_tls_stream_context.inc.cpp"
#include "platform/win32/native_tls_stream_api.inc.cpp"

} // namespace

#include "platform/win32/native_tls_stream_public.inc.cpp"

} // namespace platform
} // namespace ecnuvpn
