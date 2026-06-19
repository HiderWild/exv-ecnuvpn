#include "core/app_api/desktop_status_presenter.hpp"
#include "core/network/virtual_network_status.hpp"

#include <iostream>

namespace {

int g_probe_calls = 0;

std::vector<ecnuvpn::virtual_network::AdapterInfo>
counting_probe(const std::string &) {
  ++g_probe_calls;
  return {};
}

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

ecnuvpn::Config sample_config() {
  ecnuvpn::Config cfg;
  cfg.server = "vpn.example.edu";
  cfg.username = "student";
  cfg.mtu = 1400;
  cfg.routes = {"10.0.0.0/8"};
  return cfg;
}

bool idle_controller_status_does_not_probe_virtual_network() {
  using namespace ecnuvpn;
  using namespace ecnuvpn::virtual_network;

  g_probe_calls = 0;
  testing::set_virtual_network_probe_provider_for_testing(counting_probe);

  exv::core::TunnelStatusSnapshot snap;
  snap.phase = exv::core::TunnelPhase::Idle;
  snap.network_ready = false;
  snap.interface_name = "";

  auto status = app_api::frontend_status_from_controller_snapshot(
      snap, sample_config());

  bool ok = true;
  ok = expect(status.value("connected", true) == false,
              "idle controller status should be disconnected") &&
       ok;
  ok = expect(status.value("route_policy", std::string()) == "normal",
              "idle controller status should keep default route policy") &&
       ok;
  ok = expect(g_probe_calls == 0,
              "idle controller status must not probe virtual networks") &&
       ok;

  testing::set_virtual_network_probe_provider_for_testing(nullptr);
  return ok;
}

bool disconnected_status_does_not_probe_virtual_network() {
  using namespace ecnuvpn;
  using namespace ecnuvpn::virtual_network;

  g_probe_calls = 0;
  testing::set_virtual_network_probe_provider_for_testing(counting_probe);

  auto status = app_api::disconnected_status(sample_config());

  bool ok = true;
  ok = expect(status.value("connected", true) == false,
              "disconnected status should be disconnected") &&
       ok;
  ok = expect(status.value("route_policy", std::string()) == "normal",
              "disconnected status should keep default route policy") &&
       ok;
  ok = expect(g_probe_calls == 0,
              "disconnected status must not probe virtual networks") &&
       ok;

  testing::set_virtual_network_probe_provider_for_testing(nullptr);
  return ok;
}

bool failed_controller_status_does_not_probe_virtual_network() {
  using namespace ecnuvpn;
  using namespace ecnuvpn::virtual_network;

  g_probe_calls = 0;
  testing::set_virtual_network_probe_provider_for_testing(counting_probe);

  exv::core::TunnelStatusSnapshot snap;
  snap.phase = exv::core::TunnelPhase::Failed;
  snap.network_ready = false;
  snap.interface_name = "";

  auto status = app_api::frontend_status_from_controller_snapshot(
      snap, sample_config());

  bool ok = true;
  ok = expect(status.value("connected", true) == false,
              "failed controller status should be disconnected") &&
       ok;
  ok = expect(g_probe_calls == 0,
              "failed controller status must not probe virtual networks") &&
       ok;

  testing::set_virtual_network_probe_provider_for_testing(nullptr);
  return ok;
}

bool connected_controller_status_probes_virtual_network_once() {
  using namespace ecnuvpn;
  using namespace ecnuvpn::virtual_network;

  g_probe_calls = 0;
  testing::set_virtual_network_probe_provider_for_testing(counting_probe);

  exv::core::TunnelStatusSnapshot snap;
  snap.phase = exv::core::TunnelPhase::Connected;
  snap.network_ready = true;
  snap.interface_name = "ECNU-VPN";

  auto status = app_api::frontend_status_from_controller_snapshot(
      snap, sample_config());

  bool ok = true;
  ok = expect(status.value("connected", false) == true,
              "connected controller status should be connected") &&
       ok;
  ok = expect(g_probe_calls == 1,
              "connected controller status should probe virtual networks once") &&
       ok;

  testing::set_virtual_network_probe_provider_for_testing(nullptr);
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = disconnected_status_does_not_probe_virtual_network() && ok;
  ok = idle_controller_status_does_not_probe_virtual_network() && ok;
  ok = failed_controller_status_does_not_probe_virtual_network() && ok;
  ok = connected_controller_status_probes_virtual_network_once() && ok;
  return ok ? 0 : 1;
}
