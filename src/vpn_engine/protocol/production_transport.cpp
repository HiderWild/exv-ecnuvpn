#include "vpn_engine/protocol/production_transport.hpp"

#include "vpn_engine/protocol/cstp.hpp"
#include "vpn_engine/protocol/http.hpp"

#include <cctype>
#include <cstddef>
#include <cstdint>
#include <limits>
#include <sstream>
#include <string_view>
#include <utility>

namespace ecnuvpn {
namespace vpn_engine {
namespace protocol {

namespace {

constexpr const char *kLoginPath = "/+CSCOE+/logon.html";
constexpr const char *kCstpPath = "/CSCOT/";
constexpr const char *kDefaultUserAgent = "ECNU-VPN Native";
constexpr std::size_t kMaxHttpHeaderBytes = 64 * 1024;
constexpr std::size_t kMaxHttpBodyBytes = 16 * 1024 * 1024;

#include "vpn_engine/protocol/production_transport_redaction.inc.cpp"
#include "vpn_engine/protocol/production_transport_requests.inc.cpp"
#include "vpn_engine/protocol/production_transport_response_parse.inc.cpp"

} // namespace

#include "vpn_engine/protocol/production_transport_auth.inc.cpp"
#include "vpn_engine/protocol/production_transport_cstp.inc.cpp"
#include "vpn_engine/protocol/production_transport_read_http.inc.cpp"

} // namespace protocol
} // namespace vpn_engine
} // namespace ecnuvpn
