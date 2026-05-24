#include "platform/common/oneshot_bootstrap.hpp"

#include "platform/common/backend_resolver.hpp"
#include "utils.hpp"

#include <chrono>
#include <random>
#include <sstream>
#include <string>
#include <unistd.h>

namespace ecnuvpn {
namespace platform {
namespace {

std::string random_hex(size_t bytes) {
  std::random_device rd;
  std::ostringstream out;
  out << std::hex;
  for (size_t i = 0; i < bytes; ++i) {
    unsigned int value = rd() & 0xffU;
    if (value < 16)
      out << '0';
    out << value;
  }
  return out.str();
}

bool wait_for_helper_hello(const HelperEndpoint &endpoint) {
  for (int i = 0; i < 40; ++i) {
    nlohmann::json hello = send_helper_request(endpoint, {{"action", "hello"}});
    if (hello.value("ok", false))
      return true;
    usleep(100000);
  }
  return false;
}

std::string apple_script_string(const std::string &value) {
  std::string out = "\"";
  for (char ch : value) {
    if (ch == '\\' || ch == '"')
      out.push_back('\\');
    out.push_back(ch);
  }
  out.push_back('"');
  return out;
}

} // namespace

OneshotBackend start_oneshot_helper(const OneshotBootstrapRequest &request) {
  OneshotBackend backend;
  backend.transport = "unix-socket";

  if (request.helper_path.empty()) {
    backend.code = kOneshotNotSupportedCode;
    backend.message = "exv-helper path is not available.";
    return backend;
  }

  std::string session_id = random_hex(8);
  backend.auth_token = random_hex(32);
  backend.endpoint = "/tmp/exv-" + std::to_string(getuid()) + "-" +
                     session_id + ".sock";

  std::string command = utils::shell_quote(request.helper_path) +
                        " --oneshot --socket " +
                        utils::shell_quote(backend.endpoint) +
                        " --auth-token " +
                        utils::shell_quote(backend.auth_token) +
                        " >/dev/null 2>&1 &";
  std::string osascript =
      "osascript -e " +
      utils::shell_quote("do shell script " + apple_script_string(command) +
                         " with administrator privileges");

  int rc = utils::run_command(osascript);
  if (rc != 0) {
    backend.code = kOneshotElevationDeniedCode;
    backend.message = "Administrator authorization was cancelled or failed.";
    return backend;
  }

  if (!wait_for_helper_hello(
          HelperEndpoint{backend.endpoint, backend.auth_token})) {
    backend.code = kHelperRpcFailedCode;
    backend.message = "One-shot helper did not become ready.";
    return backend;
  }

  backend.ok = true;
  return backend;
}

} // namespace platform
} // namespace ecnuvpn
