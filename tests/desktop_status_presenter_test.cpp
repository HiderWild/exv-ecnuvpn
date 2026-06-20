#include "core/app_api/desktop_status_presenter.hpp"
#include "core/network/virtual_network_status.hpp"

#include <atomic>
#include <chrono>
#include <thread>
#include <iostream>

namespace {

std::atomic<int> g_probe_calls{0};

std::vector<exv::virtual_network::AdapterInfo>
counting_probe(const std::string &exv_interface) {
  g_probe_calls.fetch_add(1);
  if (!exv_interface.empty() && exv_interface != "EXV") {
    return {};
  }
  return {{"Mihomo", "Meta Tunnel", "proxy_tun", "internet_proxy", "38",
           "default route metric 1"}};
}

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

exv::Config sample_config() {
  exv::Config cfg;
  cfg.server = "vpn.example.edu";
  cfg.username = "student";
  cfg.mtu = 1400;
  cfg.routes = {"10.0.0.0/8"};
  return cfg;
}

bool expect_default_virtual_network_fields(const nlohmann::json &status,
                                           const char *context) {
  bool ok = true;
  ok = expect(status.value("route_policy", std::string()) == "normal",
              (std::string(context) +
               " should return cached/default route policy immediately")
                  .c_str()) &&
       ok;
  ok = expect(!status.value("upstream_virtual_detected", true),
              (std::string(context) +
               " should not block for upstream proxy TUN detection")
                  .c_str()) &&
       ok;
  return ok;
}

nlohmann::json wait_for_virtual_network_event() {
  for (int i = 0; i < 20; ++i) {
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
    auto events = exv::app_api::drain_virtual_network_status_events();
    if (events.is_array() && !events.empty()) {
      return events;
    }
  }
  return nlohmann::json::array();
}

bool expect_finished_proxy_tun_event(const nlohmann::json &events,
                                     const char *context) {
  bool ok = true;
  ok = expect(events.is_array() && events.size() == 1,
              (std::string(context) +
               " should emit exactly one async virtual network status patch")
                  .c_str()) &&
       ok;
  if (events.is_array() && events.size() == 1) {
    const auto &event = events[0];
    ok = expect(event.value("upstream_virtual_detected", false),
                (std::string(context) +
                 " should report detected proxy TUN in async patch")
                    .c_str()) &&
         ok;
    ok = expect(event.value("route_policy", std::string()) ==
                    "exv-before-proxy-tun",
                (std::string(context) +
                 " should report planned EXV-before-proxy policy")
                    .c_str()) &&
         ok;
    ok = expect(event.value("Finnished", false),
                (std::string(context) +
                 " should mark the final meaningful patch as Finnished=true")
                    .c_str()) &&
         ok;
  }
  return ok;
}

bool idle_controller_status_starts_async_virtual_network_probe() {
  using namespace exv;
  using namespace exv::virtual_network;

  g_probe_calls.store(0);
  app_api::reset_virtual_network_probe_state_for_testing();
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
  ok = expect_default_virtual_network_fields(status,
                                             "idle controller status") &&
       ok;
  ok = expect_finished_proxy_tun_event(wait_for_virtual_network_event(),
                                       "idle controller status") &&
       ok;
  ok = expect(g_probe_calls.load() == 1,
              "idle controller status should run one background probe") && ok;

  testing::set_virtual_network_probe_provider_for_testing(nullptr);
  app_api::reset_virtual_network_probe_state_for_testing();
  return ok;
}

bool disconnected_status_starts_async_virtual_network_probe() {
  using namespace exv;
  using namespace exv::virtual_network;

  g_probe_calls.store(0);
  app_api::reset_virtual_network_probe_state_for_testing();
  testing::set_virtual_network_probe_provider_for_testing(counting_probe);

  auto status = app_api::disconnected_status(sample_config());

  bool ok = true;
  ok = expect(status.value("connected", true) == false,
              "disconnected status should be disconnected") &&
       ok;
  ok = expect_default_virtual_network_fields(status, "disconnected status") &&
       ok;
  ok = expect_finished_proxy_tun_event(wait_for_virtual_network_event(),
                                       "disconnected status") &&
       ok;
  ok = expect(g_probe_calls.load() == 1,
              "disconnected status should run one background probe") && ok;

  testing::set_virtual_network_probe_provider_for_testing(nullptr);
  app_api::reset_virtual_network_probe_state_for_testing();
  return ok;
}

bool failed_controller_status_starts_async_virtual_network_probe() {
  using namespace exv;
  using namespace exv::virtual_network;

  g_probe_calls.store(0);
  app_api::reset_virtual_network_probe_state_for_testing();
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
  ok = expect_default_virtual_network_fields(status,
                                             "failed controller status") &&
       ok;
  ok = expect_finished_proxy_tun_event(wait_for_virtual_network_event(),
                                       "failed controller status") &&
       ok;
  ok = expect(g_probe_calls.load() == 1,
              "failed controller status should run one background probe") && ok;

  testing::set_virtual_network_probe_provider_for_testing(nullptr);
  app_api::reset_virtual_network_probe_state_for_testing();
  return ok;
}

bool connected_controller_status_starts_async_virtual_network_probe_once() {
  using namespace exv;
  using namespace exv::virtual_network;

  g_probe_calls.store(0);
  app_api::reset_virtual_network_probe_state_for_testing();
  testing::set_virtual_network_probe_provider_for_testing(counting_probe);

  exv::core::TunnelStatusSnapshot snap;
  snap.phase = exv::core::TunnelPhase::Connected;
  snap.network_ready = true;
  snap.interface_name = "EXV";

  auto status = app_api::frontend_status_from_controller_snapshot(
      snap, sample_config());

  bool ok = true;
  ok = expect(status.value("connected", false) == true,
              "connected controller status should be connected") &&
       ok;
  ok = expect_default_virtual_network_fields(status,
                                             "connected controller status") &&
       ok;
  ok = expect_finished_proxy_tun_event(wait_for_virtual_network_event(),
                                       "connected controller status") &&
       ok;
  ok = expect(g_probe_calls.load() == 1,
              "connected controller status should run one background probe") && ok;

  testing::set_virtual_network_probe_provider_for_testing(nullptr);
  app_api::reset_virtual_network_probe_state_for_testing();
  return ok;
}

bool virtual_network_probe_runs_asynchronously_and_drains_finished_event() {
  using namespace exv;
  using namespace exv::virtual_network;

  g_probe_calls.store(0);
  app_api::reset_virtual_network_probe_state_for_testing();
  testing::set_virtual_network_probe_provider_for_testing(
      [](const std::string &) {
        g_probe_calls.fetch_add(1);
        std::this_thread::sleep_for(std::chrono::milliseconds(80));
        return std::vector<AdapterInfo>{{"Mihomo", "Meta Tunnel", "proxy_tun",
                                         "internet_proxy", "38",
                                         "default route metric 1"}};
      });

  const auto started = std::chrono::steady_clock::now();
  auto status = app_api::disconnected_status(sample_config());
  const auto elapsed = std::chrono::duration_cast<std::chrono::milliseconds>(
      std::chrono::steady_clock::now() - started);
  auto immediate_events = app_api::drain_virtual_network_status_events();

  bool ok = true;
  ok = expect(elapsed < std::chrono::milliseconds(50),
              "disconnected status should not block on virtual probe") &&
       ok;
  ok = expect(status.value("route_policy", std::string()) == "normal",
              "initial disconnected status should use cached/default route policy") &&
       ok;
  ok = expect(immediate_events.is_array() && immediate_events.empty(),
              "drain should not emit empty frontend status responses") &&
       ok;

  nlohmann::json finished_events = nlohmann::json::array();
  for (int i = 0; i < 20; ++i) {
    std::this_thread::sleep_for(std::chrono::milliseconds(20));
    finished_events = app_api::drain_virtual_network_status_events();
    if (finished_events.is_array() && !finished_events.empty()) {
      break;
    }
  }

  ok = expect(g_probe_calls.load() == 1,
              "background virtual probe should run once") &&
       ok;
  ok = expect(finished_events.is_array() && finished_events.size() == 1,
              "drain should emit one completed virtual probe patch") &&
       ok;
  if (finished_events.is_array() && finished_events.size() == 1) {
    const auto &event = finished_events[0];
    ok = expect(event.value("upstream_virtual_detected", false),
                "finished virtual probe should include detected adapter") &&
         ok;
    ok = expect(event.value("route_policy", std::string()) ==
                    "exv-before-proxy-tun",
                "finished virtual probe should include planned route policy") &&
         ok;
    ok = expect(event.value("Finnished", false),
                "last meaningful async status patch should set Finnished=true") &&
         ok;
  }

  auto cached_status = app_api::disconnected_status(sample_config());
  ok = expect(cached_status.value("route_policy", std::string()) ==
                  "exv-before-proxy-tun",
              "later disconnected status should include cached virtual probe") &&
       ok;

  testing::set_virtual_network_probe_provider_for_testing(nullptr);
  app_api::reset_virtual_network_probe_state_for_testing();
  return ok;
}

} // namespace

int main() {
  bool ok = true;
  ok = disconnected_status_starts_async_virtual_network_probe() && ok;
  ok = idle_controller_status_starts_async_virtual_network_probe() && ok;
  ok = failed_controller_status_starts_async_virtual_network_probe() && ok;
  ok = connected_controller_status_starts_async_virtual_network_probe_once() && ok;
  ok = virtual_network_probe_runs_asynchronously_and_drains_finished_event() && ok;
  return ok ? 0 : 1;
}
