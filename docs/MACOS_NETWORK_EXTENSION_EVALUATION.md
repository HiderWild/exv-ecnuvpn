# macOS Network Extension Evaluation

> **Date**: 2026-06-03
> **Status**: Evaluation only -- no implementation changes
> **Scope**: Assess current utun/helper architecture vs. Apple Network Extension requirements

---

## 1. Executive Summary

The current macOS architecture uses a privileged helper daemon (`exv-helper`) running as root via launchd, with a utun-based tunnel interface and route/DNS configuration performed directly by the helper and supervisor processes. This architecture works for **direct DMG distribution** but is **incompatible with Mac App Store distribution**, which requires Apple's Network Extension framework.

This document evaluates:
1. The current architecture's fitness for non-App Store distribution
2. The migration scope if App Store or MDM distribution becomes necessary
3. Which corelib components are reusable under a Network Extension model
4. A recommendation summary

---

## 2. Current Architecture Assessment

### 2.1 How It Works Today

| Component | Role | Privilege Level |
|---|---|---|
| `exv` (Electron + CLI) | User-facing UI, config management | User (unprivileged) |
| `exv-helper` (launchd daemon) | IPC server, spawns supervisor/worker | Root (`/Library/LaunchDaemons/`) |
| `__vpn-supervisor` (forked child) | Holds VPN session, packet loop, reconnection | Root (inherited from helper) |
| utun interface | Kernel tunnel adapter | Created by supervisor process |
| Route/DNS config | Applied via `route` CLI and `scutil` | Root (inherited) |

### 2.2 Strengths for DMG Distribution

1. **Full control**: No Apple sandbox restrictions; direct kernel utun access
2. **Proven pattern**: Similar to how Cisco AnyConnect, OpenConnect, and other enterprise VPN clients work outside the App Store
3. **Minimal dependencies**: Uses only POSIX APIs, Security.framework, and launchd
4. **Fast iteration**: No App Store review cycle; immediate deployment
5. **Root access**: Helper runs as root, enabling direct route/DNS/interface manipulation

### 2.3 Limitations for DMG Distribution

1. **Manual installation**: User must drag DMG, run `sudo exv service install`
2. **Gatekeeper warnings**: Unsigned or self-signed builds trigger Gatekeeper alerts
3. **SIP considerations**: Some operations may conflict with System Integrity Protection
4. **No auto-update**: DMG distribution lacks built-in update mechanism (Electron autoUpdater can help)
5. **Keychain access**: First credential save triggers user approval prompt

---

## 3. Network Extension Requirements

### 3.1 What Apple Requires

For App Store distribution, VPN apps must use one of:

| Extension Type | Use Case | Tunnel Protocol |
|---|---|---|
| `NETunnelProviderManager` (PacketTunnel) | Custom tunnel protocol | IP packets over tunnel |
| `NEVPNManager` (IPSec/IKEv2) | Standard IPSec VPN | IPSec |
| `NETransparentProxyManager` | Proxy-based | TCP/UDP proxy |

For AnyConnect-style VPN: **`NETunnelProviderManager`** (PacketTunnel) is the only viable option.

### 3.2 PacketTunnel Provider Constraints

| Constraint | Impact on ECNU-VPN |
|---|---|
| Runs in a sandboxed extension process | Cannot use `fork()`, `exec()`, or arbitrary system calls |
| No root access | Must use `NEPacketTunnelNetworkSettings` for IP/DNS/routes |
| Limited network stack | Cannot directly manipulate routing table; must use `NEPacketTunnelProvider` APIs |
| Entitlements required | `com.apple.developer.networking.networkextension` + `com.apple.developer.networking.vpn.api` |
| App Review | Apple reviews all Network Extension entitlements |
| Shared container | Extension and app share data via App Group container |
| No launchd daemon | The extension IS the privileged process; no separate helper |

### 3.3 Key Incompatibilities with Current Architecture

| Current Component | NE Incompatibility | Severity |
|---|---|---|
| `exv-helper` as root launchd daemon | NE runs as sandboxed extension, not root | **Breaking** |
| `fork()` + `setsid()` for supervisor | `fork()` prohibited in sandboxed extensions | **Breaking** |
| Direct utun `ioctl()` | Must use `PacketTunnelProvider` to create tunnel | **Breaking** |
| `route` CLI for routing | Must use `NEPacketTunnelNetworkSettings` routes | **Breaking** |
| `scutil` for DNS | Must use `NEPacketTunnelNetworkSettings` DNS | **Breaking** |
| Unix socket IPC at `/var/run/` | Must use App Group container for IPC | **Breaking** |
| Keychain via `SecKeychain*` | Must use App Group Keychain or Keychain Sharing entitlement | Moderate |
| Helper whitelist model | Replaced by extension sandbox + entitlements | Moderate |
| Helper `KeepAlive` restart | NE lifecycle managed by `networkextension` daemon | Moderate |

---

## 4. Migration Scope Assessment

### 4.1 Component-by-Component Analysis

#### 4.1.1 VPN Engine (`src/vpn_engine/`) -- REUSABLE

| Component | Reusable? | Notes |
|---|---|---|
| `protocol/session.hpp/.cpp` | Yes | Protocol state machine is platform-independent |
| `protocol/production_transport.hpp/.cpp` | Yes | TLS + CSTP framing; no platform dependency |
| `protocol/auth.hpp/.cpp` | Yes | AnyConnect authentication; no platform dependency |
| `protocol/cstp.hpp/.cpp` | Yes | CSTP frame encode/decode; pure protocol |
| `protocol/http.hpp/.cpp` | Yes | HTTP parsing; pure protocol |
| `protocol/tls_stream.hpp` | Yes (interface) | Need new NE implementation of `TlsStream` |
| `protocol/url.hpp/.cpp` | Yes | URL parsing; pure protocol |
| `native_engine.hpp/.cpp` | Partial | Session lifecycle reusable; packet loop needs adaptation |
| `session_state.hpp/.cpp` | Yes | State machine; no platform dependency |
| `native_session_store.hpp/.cpp` | Partial | File-based storage; need App Group path |
| `event_sink.hpp/.cpp` | Yes | Event system; platform-independent |
| `packet_device.hpp` | Interface only | Need NE `PacketTunnelProvider` implementation |
| `native_error_contract.hpp` | Yes | Error codes; platform-independent |

**Assessment**: ~80% of the protocol layer is directly reusable. The `TlsStream` and `PacketDevice` interfaces need new implementations for NE.

#### 4.1.2 Core Logic (`src/`) -- PARTIALLY REUSABLE

| Component | Reusable? | Notes |
|---|---|---|
| `config.hpp/.cpp` | Yes (with path change) | Move to App Group container |
| `config_manager.hpp/.cpp` | Yes (with path change) | Atomic writes work in sandbox |
| `crypto.hpp/.cpp` | Yes | AES encryption; platform-independent |
| `logger.hpp/.cpp` | Partial | File logging needs App Group path |
| `vpn.hpp/.cpp` | Significant rewrite | Supervisor model incompatible with NE |
| `helper.hpp/.cpp` | **Not reusable** | Entire helper daemon concept replaced by NE extension |
| `helper_ipc.hpp` | **Not reusable** | IPC replaced by App Group or extension communication |
| `tunnel.hpp/.cpp` | **Not reusable** | Route scripts replaced by NE network settings |
| `app_api.hpp/.cpp` | Partial | RPC dispatch logic reusable; helper calls need rewrite |
| `webui.hpp/.cpp` | Yes | HTTP server; no platform dependency |
| `sse_broadcaster.hpp/.cpp` | Yes | SSE push; no platform dependency |
| `main.cpp` | Partial | CLI logic reusable; service management changes |

**Assessment**: ~50% of core logic is reusable. The helper IPC, tunnel scripts, and supervisor model need complete rework.

#### 4.1.3 macOS Platform Layer (`src/platform/darwin/`) -- MOSTLY REPLACED

| Component | Reusable? | Notes |
|---|---|---|
| `native_utun.hpp/.cpp` | **No** | NE provides tunnel via `PacketTunnelProvider` |
| `native_packet_device.hpp/.cpp` | **No** | Replaced by `NEPacketTunnelProvider` read/write |
| `native_route_config.hpp/.cpp` | **No** | Replaced by `NEPacketTunnelNetworkSettings` |
| `native_tls_stream.hpp/.cpp` | **Partially** | SecureTransport logic reusable, but extension sandbox may limit API access |
| `darwin_keychain_store.hpp/.cpp` | **Yes** | Keychain API works in extensions with entitlements |
| `helper_service_manager.cpp` | **No** | launchd daemon replaced by NE lifecycle |
| `helper_lifecycle.cpp` | **No** | Daemon lifecycle replaced by extension lifecycle |
| `helper_platform.cpp` | **No** | Platform config not applicable |
| `helper_client.cpp` | **No** | Unix socket IPC replaced by extension communication |
| `vpn_supervisor_process.cpp` | **No** | fork+setsid prohibited in sandbox |
| `openconnect_process.cpp` | **No** | External process spawning prohibited in sandbox |
| `virtual_network_probe.cpp` | **Yes** | Network interface detection; App Group accessible |
| `path_utils.cpp` | **Partial** | Paths need App Group prefix |

**Assessment**: ~20% reusable. Most macOS platform code is incompatible with NE sandbox restrictions.

#### 4.1.4 Cross-Platform Abstractions (`src/platform/common/`) -- PARTIALLY REUSABLE

| Component | Reusable? | Notes |
|---|---|---|
| `credential_store.hpp` | Yes | Abstract interface; `DarwinKeychainStore` works in NE |
| `helper_platform.hpp` | No | Helper concept replaced |
| `helper_service_manager.hpp` | No | Service management replaced by NE |
| `helper_lifecycle.hpp` | No | Lifecycle management replaced |
| `config_defaults.hpp` | Yes | Default values; platform-independent |
| `backend_resolver.hpp` | Partial | Service resolution logic; NE mode needs new resolver |
| `runtime_status.hpp` | Yes | Status JSON; platform-independent |
| `vpn_supervisor_process.hpp` | No | Supervisor model replaced |
| `driver_status.hpp` | No | WinTun/utun detection replaced |
| `virtual_network_probe.hpp` | Yes | Network detection; platform-independent |

**Assessment**: ~40% reusable. Helper-related abstractions are replaced; protocol and config abstractions survive.

### 4.2 Reusability Summary

| Layer | Current Files | Reusable | Replacement Needed |
|---|---|---|---|
| Protocol engine (`vpn_engine/protocol/`) | ~12 files | ~10 files (80%) | `TlsStream` NE impl |
| Core logic (`src/`) | ~30 files | ~15 files (50%) | Helper IPC, tunnel, supervisor |
| macOS platform (`platform/darwin/`) | ~12 files | ~3 files (20%) | utun, route, packet device, helper |
| Cross-platform (`platform/common/`) | ~20 files | ~8 files (40%) | Helper abstractions |
| **Total** | ~74 files | ~36 files (49%) | ~38 files |

### 4.3 New Code Required for NE

| New Component | Complexity | Description |
|---|---|---|
| `PacketTunnelProvider` | High | Core NE extension: `startTunnel()`, `stopTunnel()`, packet read/write |
| `NEPacketTunnelNetworkSettings` | Medium | IP, DNS, route configuration via NE API |
| `NE隧道网络设置` | Medium | Tunnel MTU, addresses, DNS servers, search domains |
| Extension sandbox entitlements | Low | Entitlements plist for Network Extension |
| App Group shared storage | Medium | Config, logs, session state in shared container |
| Extension-to-app IPC | Medium | `NETunnelProviderManager` control channel or App Group files |
| Keychain Sharing entitlement | Low | Share Keychain access between app and extension |
| Extension lifecycle management | Medium | App manages extension enable/disable via `NETunnelProviderManager` |

---

## 5. Migration Path Options

### 5.1 Option A: Keep DMG Distribution Only

**Scope**: No changes needed
**Pros**: No migration effort; full feature set; proven architecture
**Cons**: Cannot distribute via App Store; manual installation required; Gatekeeper warnings for unsigned builds
**Recommendation**: Suitable for campus/internal distribution where users are technically capable

### 5.2 Option B: Hybrid Distribution (DMG + App Store)

**Scope**: Maintain both paths
**Pros**: Reach both power users (DMG) and App Store users (NE); share protocol layer
**Cons**: Dual maintenance burden; two code paths for tunnel/network operations
**Recommendation**: Viable if App Store presence is important; requires ~38 new/replaced files

### 5.3 Option C: App Store Only (Full NE Migration)

**Scope**: Replace entire helper/supervisor/tunnel architecture
**Pros**: Clean architecture; single distribution channel; Apple review as trust signal
**Cons**: Significant effort; some features may be restricted by sandbox; loses root access
**Recommendation**: Only if App Store is the primary distribution channel

---

## 6. Recommendation Summary

### For Current Campus Distribution (Recommended: Option A)

The current architecture is **well-suited for direct DMG distribution** to a campus environment:

- Users can be guided through `sudo exv service install` once
- The root helper provides full network control needed for AnyConnect VPN
- No App Store review cycle or sandbox restrictions
- Protocol layer is clean-room and well-tested

### If App Store Distribution Becomes Required (Option B or C)

**Estimated effort**: 3-5 engineer-weeks for a minimal NE implementation

**Migration priority**:
1. Implement `PacketTunnelProvider` with basic tunnel I/O
2. Migrate `TlsStream` to work in extension sandbox
3. Replace helper IPC with `NETunnelProviderManager` control channel
4. Move config/logs to App Group shared container
5. Replace route/DNS configuration with `NEPacketTunnelNetworkSettings`
6. Add entitlements and App Group configuration
7. Update Electron app to manage NE lifecycle
8. Test and submit for App Review

**Reusable without changes**:
- All protocol encoding/decoding (CSTP, auth XML, HTTP)
- Config model and manager (with path changes)
- Crypto (AES encryption)
- Error classification and feedback
- Event sink and logging format
- WebUI and SSE broadcaster
- Keychain store (with entitlement)

**Must be rewritten**:
- Helper daemon and IPC
- Supervisor process and reconnection
- utun interface creation
- Route and DNS configuration
- Tunnel script generation
- Service install/uninstall

---

## 7. Risk Assessment

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Apple rejects NE entitlement application | Medium | High | Prepare documentation of campus VPN use case |
| NE sandbox blocks SecureTransport APIs | Low | Medium | Test early; fall back to URLSession for TLS |
| Extension process crash loses tunnel | Low | Medium | NE framework auto-restarts extension |
| App Review delays release cycle | High | Medium | Maintain DMG path as fallback |
| NE packet throughput insufficient | Low | High | Profile early; CSTP-over-TLS has overhead regardless |
| Keychain Sharing entitlement complexity | Low | Low | Standard entitlement; well-documented |

---

## 8. Conclusion

The current utun/helper/launchd architecture is **appropriate and effective** for the project's current distribution model (DMG to campus users). A Network Extension migration should only be undertaken if App Store distribution becomes a business requirement. The protocol layer (`vpn_engine/protocol/`) is highly reusable and represents the project's core intellectual property -- this investment is preserved regardless of the tunnel delivery mechanism.

**No implementation changes are recommended at this time.** This evaluation serves as a reference for future architectural decisions.
