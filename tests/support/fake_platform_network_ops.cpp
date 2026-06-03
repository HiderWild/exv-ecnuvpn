#include "fake_platform_network_ops.hpp"

namespace exv::test {

platform::TunnelDeviceDescriptor FakePlatformNetworkOps::prepare_tunnel_device(const std::string& adapter_name, int mtu) {
    prepare_count_++;
    platform::TunnelDeviceDescriptor desc;
    if (prepare_fail_ || adapter_create_fail_ || unsupported_) {
        desc.is_open = false;
        return desc;
    }
    desc.path = "//./FakeTun/" + adapter_name;
    desc.adapter_name = adapter_name;
    desc.mtu = mtu;
    desc.is_open = true;
    devices_[adapter_name] = desc;
    return desc;
}

platform::TunnelDeviceDescriptor FakePlatformNetworkOps::open_tunnel_device(const std::string& adapter_name) {
    auto it = devices_.find(adapter_name);
    if (it != devices_.end()) return it->second;
    return {};
}

bool FakePlatformNetworkOps::apply_tunnel_config(const platform::TunnelDeviceDescriptor& device, const platform::TunnelConfig& config) {
    apply_count_++;
    if (apply_fail_ || route_add_fail_ || dns_fail_ || unsupported_) return false;
    applied_configs_.push_back(config);
    return true;
}

platform::CleanupResult FakePlatformNetworkOps::cleanup(const std::string& adapter_name, platform::CleanupPolicy policy) {
    cleanup_count_++;
    platform::CleanupResult result;
    if (cleanup_fail_) {
        result.success = false;
        result.error_message = "Simulated cleanup failure";
        return result;
    }
    result.success = true;
    result.routes_removed = 2;
    result.dns_removed = true;
    result.adapter_removed = (policy == platform::CleanupPolicy::Full);
    devices_.erase(adapter_name);
    return result;
}

bool FakePlatformNetworkOps::device_exists(const std::string& adapter_name) const {
    return devices_.count(adapter_name) > 0;
}

void FakePlatformNetworkOps::set_prepare_should_fail(bool fail) { prepare_fail_ = fail; }
void FakePlatformNetworkOps::set_apply_should_fail(bool fail) { apply_fail_ = fail; }
void FakePlatformNetworkOps::set_cleanup_should_fail(bool fail) { cleanup_fail_ = fail; }
void FakePlatformNetworkOps::set_route_add_fail(bool fail) { route_add_fail_ = fail; }
void FakePlatformNetworkOps::set_dns_fail(bool fail) { dns_fail_ = fail; }
void FakePlatformNetworkOps::set_adapter_create_fail(bool fail) { adapter_create_fail_ = fail; }
void FakePlatformNetworkOps::set_unsupported(bool unsupported) { unsupported_ = unsupported; }

int FakePlatformNetworkOps::prepare_count() const { return prepare_count_; }
int FakePlatformNetworkOps::apply_count() const { return apply_count_; }
int FakePlatformNetworkOps::cleanup_count() const { return cleanup_count_; }
std::vector<platform::TunnelConfig> FakePlatformNetworkOps::applied_configs() const { return applied_configs_; }

} // namespace exv::test
