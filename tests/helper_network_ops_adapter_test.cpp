#include "helper/helper_network_ops.hpp"
#include "platform/common/platform_network_ops.hpp"

#include <iostream>
#include <memory>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

class RecordingPlatformNetworkOps final : public exv::platform::PlatformNetworkOps {
public:
  exv::platform::TunnelDeviceDescriptor prepare_tunnel_device(
      const std::string &adapter_name, int mtu = 1400) override {
    ++prepare_count;
    last_prepare_adapter = adapter_name;
    last_prepare_mtu = mtu;
    exv::platform::TunnelDeviceDescriptor descriptor;
    descriptor.path = "platform-device://" + adapter_name;
    descriptor.adapter_name = adapter_name;
    descriptor.mtu = 1320;
    descriptor.is_open = true;
    return descriptor;
  }

  exv::platform::TunnelDeviceDescriptor open_tunnel_device(
      const std::string &adapter_name) override {
    exv::platform::TunnelDeviceDescriptor descriptor;
    descriptor.adapter_name = adapter_name;
    return descriptor;
  }

  bool apply_tunnel_config(
      const exv::platform::TunnelDeviceDescriptor &device,
      const exv::platform::TunnelConfig &config) override {
    ++apply_count;
    last_apply_device = device;
    last_apply_config = config;
    return true;
  }

  exv::platform::CleanupResult cleanup(
      const std::string &adapter_name,
      exv::platform::CleanupPolicy policy) override {
    ++cleanup_count;
    last_cleanup_adapter = adapter_name;
    last_cleanup_policy = policy;
    exv::platform::CleanupResult result;
    result.success = true;
    return result;
  }

  exv::platform::CleanupResult cleanup_resources(
      const std::vector<exv::platform::ManagedNetworkResource> &resources,
      exv::platform::CleanupPolicy policy) override {
    ++cleanup_resources_count;
    last_cleanup_resources = resources;
    last_cleanup_policy = policy;
    return cleanup(resources.empty() ? std::string() : resources.front().detail,
                   policy);
  }

  bool device_exists(const std::string &adapter_name) const override {
    return adapter_name == last_prepare_adapter;
  }

  int prepare_count = 0;
  int apply_count = 0;
  int cleanup_count = 0;
  int cleanup_resources_count = 0;
  int last_prepare_mtu = 0;
  std::string last_prepare_adapter;
  std::string last_cleanup_adapter;
  exv::platform::CleanupPolicy last_cleanup_policy =
      exv::platform::CleanupPolicy::RoutesOnly;
  exv::platform::TunnelDeviceDescriptor last_apply_device;
  exv::platform::TunnelConfig last_apply_config;
  std::vector<exv::platform::ManagedNetworkResource> last_cleanup_resources;
};

int test_platform_backed_helper_ops_delegate_and_track_resources() {
  bool ok = true;
  auto platform_ops = std::make_unique<RecordingPlatformNetworkOps>();
  auto *recording = platform_ops.get();
  auto helper_ops = exv::helper::create_helper_network_ops(std::move(platform_ops));

  ok = expect(helper_ops != nullptr,
              "explicit platform backend should create helper network ops") &&
       ok;

  exv::helper::SessionId session;
  session.value = "session-a";
  exv::helper::PrepareTunnelDeviceRequest prepare_req;
  prepare_req.session_id = session;
  prepare_req.adapter_name = "EXV";
  std::vector<exv::helper::ManagedResource> prepare_resources;
  auto prepare =
      helper_ops->prepare_tunnel_device(prepare_req, &prepare_resources);

  ok = expect(prepare.device_path == "platform-device://EXV",
              "prepare should return platform device path") &&
       ok;
  ok = expect(prepare.mtu == 1320, "prepare should return platform MTU") && ok;
  ok = expect(recording->prepare_count == 1,
              "platform prepare should be called once") &&
       ok;
  ok = expect(prepare_resources.size() == 1 &&
                  prepare_resources[0].type == "adapter" &&
                  prepare_resources[0].detail == "EXV",
              "prepare should register the adapter resource") &&
       ok;

  exv::helper::ApplyTunnelConfigRequest apply_req;
  apply_req.config.session_id = session;
  apply_req.config.interface_address = "10.0.0.2/24";
  apply_req.config.routes.push_back({"10.0.0.0/8", "10.0.0.1", 10});
  apply_req.config.server_bypass_ips = {"192.0.2.10", "192.0.2.11/32"};
  apply_req.config.dns.servers = {"10.0.0.53"};
  apply_req.config.enable_kill_switch = true;
  std::vector<exv::helper::ManagedResource> apply_resources;
  auto apply = helper_ops->apply_tunnel_config(apply_req, &apply_resources);

  ok = expect(apply.success, "apply should report platform success") && ok;
  ok = expect(recording->apply_count == 1,
              "platform apply should be called once") &&
       ok;
  ok = expect(recording->last_apply_device.path ==
                  "platform-device://EXV",
              "apply should use the prepared platform descriptor") &&
       ok;
  ok = expect(recording->last_apply_config.interface_address ==
                  "10.0.0.2/24",
              "apply should forward interface address") &&
       ok;
  ok = expect(recording->last_apply_config.routes.size() == 1 &&
                  recording->last_apply_config.routes[0].destination ==
                      "10.0.0.0/8",
              "apply should forward routes") &&
       ok;
  ok = expect(recording->last_apply_config.server_bypass_ips.size() == 2 &&
                  recording->last_apply_config.server_bypass_ips[0] ==
                      "192.0.2.10" &&
                  recording->last_apply_config.server_bypass_ips[1] ==
                      "192.0.2.11/32",
              "apply should forward all server bypass IPs") &&
       ok;
  ok = expect(recording->last_apply_config.dns.servers.size() == 1 &&
                  recording->last_apply_config.dns.servers[0] == "10.0.0.53",
              "apply should forward DNS servers") &&
       ok;
  ok = expect(apply_resources.size() == 2,
              "apply should register route and DNS resources") &&
       ok;

  exv::helper::CleanupPolicy cleanup_policy;
  cleanup_policy.remove_adapter = true;
  std::vector<exv::helper::ManagedResource> cleanup_resources =
      prepare_resources;
  cleanup_resources.insert(cleanup_resources.end(), apply_resources.begin(),
                           apply_resources.end());
  auto cleanup = helper_ops->cleanup(session, cleanup_policy, cleanup_resources);

  ok = expect(cleanup.success, "cleanup should report platform success") && ok;
  ok = expect(recording->cleanup_count == 1,
              "platform cleanup fallback should be called once") &&
       ok;
  ok = expect(recording->cleanup_resources_count == 1,
              "helper cleanup should call platform resource-aware cleanup") &&
       ok;
  ok = expect(recording->last_cleanup_resources.size() == 3,
              "resource-aware cleanup should receive all managed resources") &&
       ok;
  ok = expect(recording->last_cleanup_adapter == "EXV",
              "cleanup should use tracked adapter") &&
       ok;
  ok = expect(recording->last_cleanup_policy ==
                  exv::platform::CleanupPolicy::Full,
              "remove_adapter cleanup should map to full cleanup") &&
       ok;

  return ok ? 0 : 1;
}

int test_cleanup_with_no_requested_actions_does_not_call_platform() {
  bool ok = true;
  auto platform_ops = std::make_unique<RecordingPlatformNetworkOps>();
  auto *recording = platform_ops.get();
  auto helper_ops = exv::helper::create_helper_network_ops(std::move(platform_ops));

  exv::helper::SessionId session;
  session.value = "session-a";
  exv::helper::PrepareTunnelDeviceRequest prepare_req;
  prepare_req.session_id = session;
  prepare_req.adapter_name = "EXV";
  std::vector<exv::helper::ManagedResource> resources;
  auto prepare = helper_ops->prepare_tunnel_device(prepare_req, &resources);
  ok = expect(!prepare.device_path.empty(),
              "prepare should succeed before no-op cleanup test") &&
       ok;

  exv::helper::CleanupPolicy policy;
  policy.remove_routes = false;
  policy.remove_dns = false;
  policy.remove_adapter = false;
  policy.remove_firewall_rules = false;
  auto cleanup = helper_ops->cleanup(session, policy, resources);
  ok = expect(!cleanup.success,
              "cleanup with no requested actions must keep resources for retry") &&
       ok;
  ok = expect(recording->cleanup_count == 0,
              "cleanup with no requested actions must not call platform cleanup") &&
       ok;

  return ok ? 0 : 1;
}

} // namespace

int main() {
  int failures = 0;
  failures += test_platform_backed_helper_ops_delegate_and_track_resources();
  failures += test_cleanup_with_no_requested_actions_does_not_call_platform();
  if (failures == 0) {
    std::cout << "helper_network_ops_adapter_test: all tests passed\n";
  } else {
    std::cerr << "helper_network_ops_adapter_test: " << failures
              << " test(s) FAILED\n";
  }
  return failures == 0 ? 0 : 1;
}
