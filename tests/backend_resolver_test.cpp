#include "platform/common/backend_resolver.hpp"
#include "platform/common/helper_client.hpp"

#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

ecnuvpn::platform::ServiceStatusSnapshot unavailable_service() {
  ecnuvpn::platform::ServiceStatusSnapshot status;
  status.installed = false;
  status.running = false;
  status.available = false;
  status.capabilities = {{"service_mode", false},
                         {"oneshot_mode", true},
                         {"temporary_connect", true}};
  return status;
}

} // namespace

namespace ecnuvpn {
namespace logger {

void info(const std::string &) {}
void warn(const std::string &) {}
void error(const std::string &) {}

} // namespace logger
namespace platform {

ServiceStatusSnapshot current_service_status() { return unavailable_service(); }

OneshotBackend start_oneshot_helper(const OneshotBootstrapRequest &request) {
  OneshotBackend backend;
  backend.ok = !request.helper_path.empty();
  backend.transport = "test";
  backend.endpoint = "test-endpoint";
  backend.auth_token = "test-token";
  backend.pid = 42;
  return backend;
}

} // namespace platform
} // namespace ecnuvpn

int main() {
  using namespace ecnuvpn::platform;

  bool ok = true;
  int oneshot_calls = 0;

  ServiceStatusSnapshot service;
  service.installed = true;
  service.running = true;
  service.available = true;
  service.endpoint = "\\\\.\\pipe\\exv-helper";
  service.capabilities = {{"service_mode", true}, {"oneshot_mode", true}};

  BackendResolverDeps service_deps{
      [&service]() { return service; },
      [&oneshot_calls](const OneshotBootstrapRequest &) {
        ++oneshot_calls;
        return OneshotBackend{};
      },
  };

  nlohmann::json resolved = resolve_backend(BackendResolveOptions{}, service_deps);
  ok = expect(resolved.value("ok", false),
              "available service should resolve successfully") &&
       ok;
  ok = expect(resolved.value("backend", std::string()) == "service",
              "available service should be selected before oneshot") &&
       ok;
  ok = expect(resolved.value("transport", std::string()) == "named-pipe",
              "Windows helper endpoint should classify as named pipe") &&
       ok;
  ok = expect(oneshot_calls == 0,
              "service resolution should not start oneshot helper") &&
       ok;

  BackendResolverDeps unavailable_deps{
      unavailable_service,
      [&oneshot_calls](const OneshotBootstrapRequest &request) {
        ++oneshot_calls;
        OneshotBackend backend;
        backend.ok = true;
        backend.transport = "unix-socket";
        backend.endpoint = request.helper_path + ".sock";
        backend.auth_token = "token";
        backend.pid = 123;
        return backend;
      },
  };

  BackendResolveOptions service_only;
  service_only.preferred_mode = "service";
  resolved = resolve_backend(service_only, unavailable_deps);
  ok = expect(!resolved.value("ok", true),
              "missing requested service should fail resolution") &&
       ok;
  ok = expect(resolved.value("code", std::string()) == kServiceNotInstalledCode,
              "missing requested service should use service_not_installed") &&
       ok;

  BackendResolveOptions probe_oneshot;
  probe_oneshot.allow_oneshot = true;
  probe_oneshot.start_oneshot = false;
  resolved = resolve_backend(probe_oneshot, unavailable_deps);
  ok = expect(!resolved.value("ok", true),
              "oneshot probe should not imply a running backend") &&
       ok;
  ok = expect(resolved.value("code", std::string()) == kOneshotNotSupportedCode,
              "oneshot probe should be explicit about deferred startup") &&
       ok;

  BackendResolveOptions start_oneshot;
  start_oneshot.preferred_mode = "oneshot";
  start_oneshot.start_oneshot = true;
  start_oneshot.helper_path = "/tmp/exv-helper";
  resolved = resolve_backend(start_oneshot, unavailable_deps);
  ok = expect(resolved.value("ok", false),
              "explicit oneshot request should return bootstrap backend") &&
       ok;
  ok = expect(resolved.value("backend", std::string()) == "oneshot",
              "explicit oneshot request should select oneshot backend") &&
       ok;
  ok = expect(resolved.value("endpoint", std::string()) ==
                  "/tmp/exv-helper.sock",
              "oneshot bootstrap should receive helper path") &&
       ok;
  ok = expect(resolved.value("auth_token", std::string()) == "token",
              "oneshot backend should expose auth token to controller") &&
       ok;

  nlohmann::json wrapped =
      backend_unavailable_error(resolved, "fallback message");
  ok = expect(wrapped.value("code", std::string()) == kHelperUnavailableCode,
              "available backend without code should wrap with helper code") &&
       ok;
  ok = expect(wrapped.contains("backend_resolution"),
              "wrapped error should preserve backend resolution details") &&
       ok;

  return ok ? 0 : 1;
}
