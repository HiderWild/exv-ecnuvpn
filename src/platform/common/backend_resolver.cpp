#include "platform/common/backend_resolver.hpp"

#include "platform/common/helper_client.hpp"
#include "platform/common/oneshot_bootstrap.hpp"
#include "platform/common/service_status.hpp"
#include "observability/log_facade.hpp"

#include <algorithm>
#include <cctype>
#include <filesystem>
#include <string>

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
                        {"pid", -1},
                        {"service", service_status_to_json(service)},
                        {"capabilities", service.capabilities}};
}

std::string normalized_helper_path(std::string path) {
  if (path.empty())
    return {};

  try {
    path = std::filesystem::path(path).lexically_normal().string();
  } catch (...) {
  }

  std::replace(path.begin(), path.end(), '\\', '/');
#ifdef _WIN32
  std::transform(path.begin(), path.end(), path.begin(),
                 [](unsigned char ch) {
                   return static_cast<char>(std::tolower(ch));
                 });
#endif
  return path;
}

bool service_matches_current_helper(const ServiceStatusSnapshot &service,
                                    const BackendResolveOptions &options) {
  if (options.helper_path.empty() || service.path.empty())
    return true;
  return normalized_helper_path(service.path) ==
         normalized_helper_path(options.helper_path);
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
  exv::observability::LogFacade::info("Backend resolver: Starting resolution - preferred_mode=" +
               options.preferred_mode + " allow_oneshot=" +
               (options.allow_oneshot ? "true" : "false"));

  ServiceStatusSnapshot service = deps.current_service_status();

  exv::observability::LogFacade::info("Backend resolver: Service status - installed=" +
               std::string(service.installed ? "true" : "false") +
               " available=" + std::string(service.available ? "true" : "false") +
               " endpoint=" + service.endpoint);

  if (options.preferred_mode == "service" ||
      options.preferred_mode == "auto") {
    if (service.available) {
      const bool stale_service_for_current_package =
          options.preferred_mode == "auto" &&
          options.allow_oneshot &&
          options.start_oneshot &&
          !service_matches_current_helper(service, options);
      if (stale_service_for_current_package) {
        exv::observability::LogFacade::warn(
            "Backend resolver: Skipping available service because its helper "
            "binary does not match the current package - service_path=" +
            service.path + " helper_path=" + options.helper_path);
      } else {
      exv::observability::LogFacade::info("Backend resolver: Using service backend - endpoint=" + service.endpoint);
      return descriptor_from_service(service);
      }
    }

    if (options.preferred_mode == "service") {
      if (!service.installed) {
        exv::observability::LogFacade::warn("Backend resolver: Service not installed");
        return unavailable(kServiceNotInstalledCode,
                           "Helper service is not installed.", service);
      }
      exv::observability::LogFacade::warn("Backend resolver: Service installed but not running");
      return unavailable(kServiceInstalledNotRunningCode,
                         "Helper service is installed but not running.",
                         service);
    }
  }

  if ((options.allow_oneshot || options.preferred_mode == "oneshot") &&
      options.start_oneshot) {
    exv::observability::LogFacade::info("Backend resolver: Starting oneshot helper - path=" + options.helper_path);
    OneshotBackend backend =
        deps.start_oneshot_helper(OneshotBootstrapRequest{options.helper_path});
    if (backend.ok) {
      exv::observability::LogFacade::info("Backend resolver: Oneshot helper started - endpoint=" +
                   backend.endpoint + " pid=" + std::to_string(backend.pid));
    } else {
      exv::observability::LogFacade::error("Backend resolver: Oneshot helper failed - code=" +
                    backend.code + " message=" + backend.message);
    }
    return oneshot_backend_to_json(backend);
  }

  if (options.allow_oneshot || options.preferred_mode == "oneshot") {
    exv::observability::LogFacade::warn("Backend resolver: Oneshot not supported without start_oneshot=true");
    return unavailable(kOneshotNotSupportedCode,
                       "One-shot helper is available only when explicitly requested.",
                       service);
  }

  if (!service.installed) {
    exv::observability::LogFacade::warn("Backend resolver: No backend available - service not installed");
    return unavailable(kServiceNotInstalledCode,
                       "Helper service is not installed.", service);
  }
  exv::observability::LogFacade::warn("Backend resolver: No backend available - service not running");
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
