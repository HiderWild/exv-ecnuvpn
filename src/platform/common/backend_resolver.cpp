#include "platform/common/backend_resolver.hpp"

#include "platform/common/helper_client.hpp"
#include "platform/common/oneshot_bootstrap.hpp"
#include "platform/common/service_status.hpp"

namespace ecnuvpn {
namespace platform {
namespace {

nlohmann::json descriptor_from_service(const ServiceStatusSnapshot &service) {
  return nlohmann::json{{"ok", true},
                        {"backend", "service"},
                        {"mode", "service"},
                        {"transport",
                         service.endpoint.rfind("\\\\.\\pipe\\", 0) == 0
                             ? "named-pipe"
                             : "unix-socket"},
                        {"endpoint", service.endpoint},
                        {"auth_required", false},
                        {"pid", nullptr},
                        {"service", service_status_to_json(service)},
                        {"capabilities", service.capabilities}};
}

nlohmann::json unavailable(const char *code, const std::string &message,
                           const ServiceStatusSnapshot &service) {
  return nlohmann::json{{"ok", false},
                        {"code", code},
                        {"message", message},
                        {"service", service_status_to_json(service)},
                        {"capabilities", service.capabilities}};
}

} // namespace

nlohmann::json resolve_backend(const BackendResolveOptions &options) {
  return resolve_backend(
      options, BackendResolverDeps{current_service_status, start_oneshot_helper});
}

nlohmann::json resolve_backend(const BackendResolveOptions &options,
                               const BackendResolverDeps &deps) {
  ServiceStatusSnapshot service = deps.current_service_status();

  if (options.preferred_mode == "service" ||
      options.preferred_mode == "auto") {
    if (service.available) {
      return descriptor_from_service(service);
    }

    if (options.preferred_mode == "service") {
      if (!service.installed) {
        return unavailable(kServiceNotInstalledCode,
                           "Helper service is not installed.", service);
      }
      return unavailable(kServiceInstalledNotRunningCode,
                         "Helper service is installed but not running.",
                         service);
    }
  }

  if ((options.allow_oneshot || options.preferred_mode == "oneshot") &&
      options.start_oneshot) {
    OneshotBackend backend =
        deps.start_oneshot_helper(OneshotBootstrapRequest{options.helper_path});
    return oneshot_backend_to_json(backend);
  }

  if (options.allow_oneshot || options.preferred_mode == "oneshot") {
    return unavailable(kOneshotNotSupportedCode,
                       "One-shot helper is available only when explicitly requested.",
                       service);
  }

  if (!service.installed) {
    return unavailable(kServiceNotInstalledCode,
                       "Helper service is not installed.", service);
  }
  return unavailable(kServiceInstalledNotRunningCode,
                     "Helper service is installed but not running.", service);
}

nlohmann::json backend_unavailable_error(const nlohmann::json &resolved,
                                         const std::string &fallback_message) {
  return nlohmann::json{
      {"ok", false},
      {"error", resolved.value("message", fallback_message)},
      {"message", resolved.value("message", fallback_message)},
      {"code", resolved.value("code", kHelperUnavailableCode)},
      {"backend_resolution", resolved},
  };
}

} // namespace platform
} // namespace ecnuvpn
