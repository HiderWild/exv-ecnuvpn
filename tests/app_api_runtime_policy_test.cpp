#include "platform/common/app_api_runtime_policy.hpp"
#include "platform/common/helper_client.hpp"
#include "utils.hpp"

#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

bool g_checked_root = false;
bool g_runtime_owner_updated = false;
bool g_runtime_path_overridden = false;
bool g_fix_config_dir_ownership_called = false;

} // namespace

namespace ecnuvpn {
namespace utils {

bool check_root() {
  g_checked_root = true;
  return false;
}

std::string get_effective_home() { return ""; }
std::string get_config_dir() { return ""; }
void set_runtime_owner(uid_t, gid_t) { g_runtime_owner_updated = true; }
void set_runtime_path_override(const std::string &, const std::string &) {
  g_runtime_path_overridden = true;
}
bool fix_config_dir_ownership() {
  g_fix_config_dir_ownership_called = true;
  return true;
}

} // namespace utils
} // namespace ecnuvpn

int main() {
  bool ok = true;

  ok = expect(std::string(ecnuvpn::platform::kHelperUnavailableCode) ==
                  "helper_unavailable",
              "helper unavailable code should remain stable") &&
       ok;

#ifdef _WIN32
  ok = expect(
           ecnuvpn::platform::helper_unavailable_connect_message() ==
               "Helper daemon is not available. Install the helper service from Settings or run 'exv service install' as Administrator.",
           "Windows connect remediation message should stay stable") &&
       ok;
  ok = expect(
           ecnuvpn::platform::helper_unavailable_disconnect_message() ==
               "Helper daemon is not available. Use the elevated desktop action or install the helper service from Settings.",
           "Windows disconnect remediation message should stay stable") &&
       ok;
#elif defined(__APPLE__)
  ok = expect(
           ecnuvpn::platform::helper_unavailable_connect_message() ==
               "Helper daemon is not available. The desktop app can request one-time administrator authorization, or you can install the helper service for persistent connections.",
           "macOS connect remediation message should stay stable") &&
       ok;
  ok = expect(
           ecnuvpn::platform::helper_unavailable_disconnect_message() ==
               "Helper daemon is not available. The desktop app can request one-time administrator authorization to disconnect this session, or you can install the helper service.",
           "macOS disconnect remediation message should stay stable") &&
       ok;
#else
  ok = expect(
           ecnuvpn::platform::helper_unavailable_connect_message() ==
               "Helper daemon is not available. Install the helper service before starting the desktop client.",
           "Linux connect remediation message should stay stable") &&
       ok;
  ok = expect(
           ecnuvpn::platform::helper_unavailable_disconnect_message() ==
               "Helper daemon is not available. Install the helper service before disconnecting managed sessions.",
           "Linux disconnect remediation message should stay stable") &&
       ok;
#endif

  ecnuvpn::platform::prepare_direct_fallback_runtime();

  #ifdef _WIN32
    ok = expect(!g_checked_root,
      "Windows runtime policy should remain an explicit no-op") &&
    ok;
  #else
  ok = expect(g_checked_root,
              "runtime policy should consult the runtime owner preflight hook") &&
       ok;
  #endif
  ok = expect(!g_runtime_owner_updated,
              "runtime policy should not mutate runtime ownership when root preflight fails") &&
       ok;
  ok = expect(!g_runtime_path_overridden,
              "runtime policy should not override runtime paths when root preflight fails") &&
       ok;
  ok = expect(!g_fix_config_dir_ownership_called,
              "runtime policy should return early before ownership repair when root preflight fails") &&
       ok;

  return ok ? 0 : 1;
}