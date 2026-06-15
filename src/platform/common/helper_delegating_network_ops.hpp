#pragma once
#include "platform/common/platform_network_ops.hpp"
#include "helper/common/helper_client.hpp"
#include "helper/common/helper_messages.hpp"
#include <memory>
#include <string>

namespace exv::platform {

/// PlatformNetworkOps implementation that delegates all privileged operations
/// to the helper daemon via HelperClient.
///
/// The core process never performs privileged network operations directly;
/// every call is forwarded to the elevated helper through IPC.
///
/// Session management: callers must invoke set_session() with a valid
/// SessionId (obtained from HelperClient::start_session) before calling
/// prepare_tunnel_device, apply_tunnel_config, or cleanup.  This matches
/// the TunnelController lifecycle: start_session -> prepare -> apply -> cleanup -> end_session.
class HelperDelegatingPlatformNetworkOps : public PlatformNetworkOps {
public:
    /// Construct with a non-owning pointer to the helper client.
    /// The helper must outlive this object.
    explicit HelperDelegatingPlatformNetworkOps(helper::HelperClient* helper);

    ~HelperDelegatingPlatformNetworkOps() override;

    // -- PlatformNetworkOps interface --

    TunnelDeviceDescriptor prepare_tunnel_device(const std::string& adapter_name, int mtu = 1400) override;
    TunnelDeviceDescriptor open_tunnel_device(const std::string& adapter_name) override;
    bool apply_tunnel_config(const TunnelDeviceDescriptor& device, const TunnelConfig& config) override;
    CleanupResult cleanup(const std::string& adapter_name, CleanupPolicy policy) override;
    bool device_exists(const std::string& adapter_name) const override;

    // -- Session management --

    /// Set the active session ID.  Must be called before any operation that
    /// requires a session (prepare, apply, cleanup).
    void set_session(const helper::SessionId& session_id);

    /// Clear the active session (e.g. after end_session).
    void clear_session();

    /// Return the current session ID (empty if not set).
    const helper::SessionId& session_id() const;

private:
    helper::HelperClient* helper_;
    helper::SessionId session_id_;
    TunnelDeviceDescriptor last_prepared_device_;
};

} // namespace exv::platform
