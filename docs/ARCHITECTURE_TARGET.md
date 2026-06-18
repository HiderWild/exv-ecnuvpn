# Architecture Target State

This document defines the target architecture for the ECNU-VPN client after the native-only cutover.

## Production Connection Owner

The production connection owner is the Core process through `TunnelController` and `NativeVpnEngineSession`.
There is no production supervisor entrypoint and no external VPN runtime fallback in the target architecture.

**Key points:**

- `TunnelController` manages VPN session lifecycle, retries, and state transitions.
- `NativeVpnEngineSession` owns AnyConnect protocol execution and packet forwarding.
- The helper remains a privileged broker for packet-device and network-configuration operations.
- New features, retry logic, and diagnostics must target the Core process / TunnelController path.

## Native Engine: Core-Owned Mode

The native VPN engine uses a long-running Core process that owns the VPN tunnel directly:

- `TunnelController` manages connection lifecycle, retries, and state transitions.
- The Core process holds the tunnel session for its entire lifetime.
- The desktop app communicates with Core via RPC; no separate connection-owner process is needed.
- Session state is managed through `native-session-state.json` and Core's internal state machine.

## Guidelines

1. All session management, retry, or diagnostic features belong in the Core process or TunnelController.
2. Helper code may prepare, configure, and clean privileged network resources, but it must not own protocol lifecycle.
3. Native AnyConnect over CSTP is the only supported production engine.
