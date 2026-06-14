#include "helper_delegating_network_ops.hpp"
#include <stdexcept>
#include <sstream>

namespace exv::platform {

// ---------------------------------------------------------------------------
// Construction / destruction
// ---------------------------------------------------------------------------

HelperDelegatingPlatformNetworkOps::HelperDelegatingPlatformNetworkOps(
    helper::HelperClient* helper)
    : helper_(helper)
{
}

HelperDelegatingPlatformNetworkOps::~HelperDelegatingPlatformNetworkOps() = default;

// ---------------------------------------------------------------------------
// Session management
// ---------------------------------------------------------------------------

void HelperDelegatingPlatformNetworkOps::set_session(const helper::SessionId& session_id) {
    session_id_ = session_id;
}

void HelperDelegatingPlatformNetworkOps::clear_session() {
    session_id_ = helper::SessionId{};
    last_prepared_device_ = TunnelDeviceDescriptor{};
}

const helper::SessionId& HelperDelegatingPlatformNetworkOps::session_id() const {
    return session_id_;
}

// ---------------------------------------------------------------------------
// Type conversion helpers (platform <-> helper)
// ---------------------------------------------------------------------------

namespace {

helper::RouteEntry to_helper_route(const platform::RouteEntry& r) {
    helper::RouteEntry hr;
    hr.destination = r.destination;
    hr.gateway = r.gateway;
    hr.metric = r.metric;
    return hr;
}

helper::DnsConfig to_helper_dns(const platform::DnsConfig& d) {
    helper::DnsConfig hd;
    hd.servers = d.servers;
    hd.search_domain = d.search_domain;
    // helper protocol does not carry suffixes; drop them
    return hd;
}

helper::CleanupPolicy to_helper_cleanup_policy(platform::CleanupPolicy policy) {
    helper::CleanupPolicy hp;
    switch (policy) {
        case platform::CleanupPolicy::Full:
            hp.remove_routes = true;
            hp.remove_dns = true;
            hp.remove_adapter = true;
            hp.remove_firewall_rules = true;
            break;
        case platform::CleanupPolicy::KeepAdapter:
            hp.remove_routes = true;
            hp.remove_dns = true;
            hp.remove_adapter = false;
            hp.remove_firewall_rules = true;
            break;
        case platform::CleanupPolicy::RoutesOnly:
            hp.remove_routes = true;
            hp.remove_dns = false;
            hp.remove_adapter = false;
            hp.remove_firewall_rules = false;
            break;
        case platform::CleanupPolicy::DnsOnly:
            hp.remove_routes = false;
            hp.remove_dns = true;
            hp.remove_adapter = false;
            hp.remove_firewall_rules = false;
            break;
    }
    return hp;
}

platform::CleanupResult from_helper_cleanup_response(const helper::CleanupResponse& resp) {
    platform::CleanupResult result;
    result.success = resp.success;
    if (!resp.errors.empty()) {
        std::ostringstream oss;
        for (size_t i = 0; i < resp.errors.size(); ++i) {
            if (i > 0) oss << "; ";
            oss << resp.errors[i];
        }
        result.error_message = oss.str();
    }
    // The helper protocol does not provide per-resource counts, so we
    // set coarse-grained flags based on success.  The caller can inspect
    // error_message for details.
    return result;
}

} // anonymous namespace

// ---------------------------------------------------------------------------
// PlatformNetworkOps interface
// ---------------------------------------------------------------------------

TunnelDeviceDescriptor HelperDelegatingPlatformNetworkOps::prepare_tunnel_device(
    const std::string& adapter_name, int mtu)
{
    if (!helper_) {
        TunnelDeviceDescriptor desc;
        desc.is_open = false;
        return desc;
    }

    helper::PrepareTunnelDeviceRequest req;
    req.session_id = session_id_;
    req.adapter_name = adapter_name;

    auto resp = helper_->prepare_tunnel_device(req);

    TunnelDeviceDescriptor desc;
    desc.path = resp.device_path;
    desc.adapter_name = adapter_name;
    desc.mtu = resp.mtu > 0 ? resp.mtu : mtu;
    desc.is_open = !resp.device_path.empty();
    // fd and handle remain at their defaults (-1 / nullptr) because the
    // actual file descriptor / HANDLE acquisition is a data-plane concern
    // handled separately (e.g. via OpenTunnelDevice on the helper or a
    // shared-memory transport).

    last_prepared_device_ = desc;
    return desc;
}

TunnelDeviceDescriptor HelperDelegatingPlatformNetworkOps::open_tunnel_device(
    const std::string& adapter_name)
{
    // The helper protocol does not have an explicit "open for I/O" operation.
    // Return the descriptor from the most recent prepare_tunnel_device call
    // if it matches the requested adapter name.  The caller is responsible
    // for obtaining the actual HANDLE/fd through the appropriate data-plane
    // transport (e.g. Wintun shared memory ring).
    if (last_prepared_device_.adapter_name == adapter_name &&
        last_prepared_device_.is_open) {
        return last_prepared_device_;
    }
    TunnelDeviceDescriptor desc;
    desc.adapter_name = adapter_name;
    return desc;
}

bool HelperDelegatingPlatformNetworkOps::apply_tunnel_config(
    const TunnelDeviceDescriptor& /*device*/, const TunnelConfig& config)
{
    if (!helper_) return false;

    helper::ApplyTunnelConfigRequest req;
    req.config.session_id = session_id_;
    req.config.interface_address = config.interface_address;
    req.config.enable_kill_switch = config.enable_kill_switch;

    for (const auto& route : config.routes) {
        req.config.routes.push_back(to_helper_route(route));
    }
    req.config.dns = to_helper_dns(config.dns);

    auto resp = helper_->apply_tunnel_config(req);
    return resp.success;
}

CleanupResult HelperDelegatingPlatformNetworkOps::cleanup(
    const std::string& /*adapter_name*/, CleanupPolicy policy)
{
    if (!helper_) {
        CleanupResult result;
        result.error_message = "No helper client";
        return result;
    }

    helper::CleanupRequest req;
    req.session_id = session_id_;
    req.policy = to_helper_cleanup_policy(policy);

    auto resp = helper_->cleanup(req);
    return from_helper_cleanup_response(resp);
}

bool HelperDelegatingPlatformNetworkOps::device_exists(
    const std::string& adapter_name) const
{
    // A device "exists" if we successfully prepared one with this name.
    // The helper protocol does not have a separate existence check.
    return last_prepared_device_.is_open &&
           last_prepared_device_.adapter_name == adapter_name;
}

} // namespace exv::platform
