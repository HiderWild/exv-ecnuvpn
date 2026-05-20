#include "platform/common/status_models.hpp"

#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

} // namespace

int main() {
  bool ok = true;

  ok = expect(ecnuvpn::platform::runtime_source_from_paths(
                  "C:/bundle/openconnect.exe",
                  "C:/bundle/openconnect.exe",
                  "C:/Windows/System32/openconnect.exe") == "bundled",
              "runtime source should classify bundled path") &&
       ok;
  ok = expect(ecnuvpn::platform::runtime_source_from_paths(
                  "/usr/local/bin/openconnect",
                  "/Applications/ECNU-VPN/openconnect",
                  "/usr/local/bin/openconnect") == "system",
              "runtime source should classify system path") &&
       ok;
  ok = expect(ecnuvpn::platform::runtime_source_from_paths(
                  "",
                  "/Applications/ECNU-VPN/openconnect",
                  "/usr/local/bin/openconnect") == "missing",
              "runtime source should report missing when no path resolves") &&
       ok;

  ecnuvpn::platform::ServiceStatusSnapshot status;
  status.installed = true;
  status.running = false;
  status.available = true;
  status.mode = "launchd";
  status.path = "/usr/local/bin/exv";
  status.endpoint = "/var/run/exv-helper.sock";
  status.label = "com.ecnu.exv.helper";
  status.warning = "waiting for helper";
  status.has_service_state = true;
  status.service_state = 4;

  nlohmann::json json = ecnuvpn::platform::service_status_to_json(status);
  ok = expect(json.value("installed", false),
              "service status should preserve installed flag") &&
       ok;
  ok = expect(!json.value("running", true),
              "service status should preserve running flag") &&
       ok;
  ok = expect(json.value("available", false),
              "service status should preserve available flag") &&
       ok;
  ok = expect(json.value("mode", std::string()) == "launchd",
              "service status should expose the platform mode") &&
       ok;
  ok = expect(json.value("path", std::string()) == "/usr/local/bin/exv",
              "service status should expose the service binary path") &&
       ok;
  ok = expect(json.value("endpoint", std::string()) ==
                  "/var/run/exv-helper.sock",
              "service status should expose the helper endpoint") &&
       ok;
  ok = expect(json.value("label", std::string()) == "com.ecnu.exv.helper",
              "service status should expose the platform label") &&
       ok;
  ok = expect(json.value("service_state", -1) == 4,
              "service status should expose an optional service state") &&
       ok;
  ok = expect(json.value("warning", std::string()) == "waiting for helper",
              "service status should preserve warnings") &&
       ok;

  return ok ? 0 : 1;
}