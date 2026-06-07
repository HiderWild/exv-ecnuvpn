# Platform Convergence — Contract Freeze (S1)

**Frozen at:** 2026-05-21
**Branch:** macos (post S0 themed commits)
**Status:** FROZEN — no changes to these interfaces without integration lead approval

## Native ↔ Electron Contract

### RPC Action Set (desktop-contract.ts)

| Action | Payload | Response | Notes |
|--------|---------|----------|-------|
| `connect` | `{ server, username, password }` | `void` | Initiates VPN connection |
| `disconnect` | `{}` | `void` | Tears down active session |
| `getStatus` | `{}` | `VpnStatus` | Polls current status |
| `getLogs` | `{}` | `string[]` | Retrieves recent log lines |
| `getVersion` | `{}` | `{ version: string }` | Returns app version |

### Status Model (VpnStatus)

```typescript
interface VpnStatus {
  state: 'disconnected' | 'connecting' | 'connected' | 'disconnecting' | 'error';
  server?: string;
  username?: string;
  duration?: number;       // seconds
  bytesIn?: number;
  bytesOut?: number;
  error?: VpnError;
}

interface VpnError {
  code: string;            // e.g. 'AUTH_FAILED', 'TLS_ERROR', 'TIMEOUT'
  message: string;
  retryable: boolean;
}
```

### IPC Channel Names

| Channel | Direction | Type |
|---------|-----------|------|
| `vpn:status` | native → renderer | VpnStatus (push) |
| `vpn:log` | native → renderer | string (push) |
| `vpn:action` | renderer → native | RpcAction (request) |
| `vpn:response` | native → renderer | RpcResponse (reply) |

## Native Platform Adapter Contract

### Headers (src/platform/common/*.hpp)

| Header | Purpose | Key Types |
|--------|---------|-----------|
| `config_defaults.hpp` | Platform-specific defaults | `ConfigDefaults` struct |
| `path_utils.hpp` | FS path resolution | `get_config_dir()`, `get_runtime_dir()`, `get_helper_path()` |
| `helper_client.hpp` | Privileged helper IPC | `HelperClient` class |
| `helper_lifecycle.hpp` | Helper install/remove | `install_helper()`, `remove_helper()` |
| `helper_service_manager.hpp` | Service lifecycle | `start_service()`, `stop_service()`, `get_service_status()` |
| `helper_platform.hpp` | Platform detection | `is_helper_available()`, `get_helper_version()` |
| `openconnect_process.hpp` | VPN process management | `OpenConnectProcess` class |
| `process_control.hpp` | Process spawn/kill | `spawn_process()`, `terminate_process()` |
| `tunnel_script.hpp` | Route script emission | `emit_tunnel_script()`, `cleanup_tunnel_script()` |
| `virtual_network_probe.hpp` | Network interface probe | `probe_vpn_interface()` |
| `vpn_supervisor_process.hpp` | VPN supervisor | `VpnSupervisorProcess` class |
| `crypto_backend.hpp` | Crypto operations | `derive_key()`, `encrypt()`, `decrypt()` |
| `app_api_runtime_policy.hpp` | Runtime policy checks | `check_runtime_policy()` |
| `runtime_status.hpp` | Aggregated status | `RuntimeStatus` class |
| `service_status.hpp` | Service status query | `ServiceStatus` enum/struct |
| `driver_status.hpp` | Driver/TUN status | `DriverStatus` struct |
| `status_models.hpp` | Unified status models | `VpnState`, `VpnError`, `ConnectionStats` |
| `helper_service_manager.hpp` | Service manager | `HelperServiceManager` class |

### Status Enums (status_models.hpp)

```cpp
enum class VpnState {
    Disconnected,
    Connecting,
    Connected,
    Disconnecting,
    Error
};

enum class ServiceState {
    Unknown,
    Running,
    Stopped,
    Error
};
```

## Freeze Rules

1. **No new RPC actions** without updating desktop-contract.ts AND all platform runners
2. **No new VpnStatus fields** without updating status_models.hpp AND desktop-contract.ts
3. **No new platform adapter headers** without integration lead review
4. **No signature changes** to existing adapter functions
5. **Error codes** must be added to both native `VpnError` and TS `VpnError.code`

## Verification

- `grep -r 'vpn:action\|vpn:status\|vpn:log\|vpn:response' webui/src/` must match desktop-contract.ts
- All `src/platform/{darwin,linux,win32}/*.cpp` must implement every pure-virtual from their `common/*.hpp`
- `npm run build` in webui/ must pass with frozen contract
- `cmake --build --preset macos-release` must pass with frozen headers
