# OpenConnect Native Replacement Previous Plan Closure

> Status: CLOSED / SUPERSEDED.
> Date: 2026-05-31.
> Superseded by: `docs/superpowers/plans/2026-05-31-native-openconnect-replacement-phase2.md`.
> Current successor: `docs/superpowers/plans/2026-05-31-native-openconnect-replacement-phase3-production-readiness.md`.

## Closure Decision

The previous OpenConnect native replacement plan is closed as an execution document.

It produced the first executable slice:

- `src/vpn_engine/engine.hpp`
- `src/vpn_engine/native_engine.hpp`
- `src/vpn_engine/native_engine.cpp`
- `Config.vpn_engine = native | legacy_openconnect`
- native runtime status that no longer requires OpenConnect
- native validation that rejects legacy `extra_args`
- focused tests for native engine contracts and native runtime status

It did not produce a working native VPN tunnel. The remaining work is now transferred into the phase 2 implementation plan.

## Transferred Work

The following unfinished or revised items are no longer tracked here:

- clean-room AnyConnect behavior specification
- ECNU username/password auth flow
- CSTP-over-TLS protocol core
- keepalive, DPD, reconnect, disconnect
- packet I/O abstraction
- Windows Wintun adapter control
- Windows IP Helper address, route, MTU, and cleanup logic
- macOS utun adapter control
- macOS route, MTU, and cleanup logic
- helper/supervisor structured session state
- removal of `find_openconnect_pid()` as the primary status source
- removal of log scraping and route-ready as the primary readiness source
- UTF-8 structured engine events
- production packaging without OpenConnect binaries and DLLs
- fake AnyConnect integration server
- Windows and macOS manual acceptance without OpenConnect installed

## Historical Old-Plan Items Reclassified

Older release-hardening plans that require staged OpenConnect binaries are historical for the native replacement lane. Their OpenConnect runtime packaging requirements are replaced by the phase 2 plan. Legacy OpenConnect fallback remains development-only until the phase 2 plan explicitly removes production packaging dependencies.
