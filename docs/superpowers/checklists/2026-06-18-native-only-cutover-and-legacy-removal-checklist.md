# Native-Only Cutover and Legacy Removal Checklist

Use this checklist with `docs/superpowers/plans/2026-06-18-native-only-cutover-and-legacy-removal-plan.md`.

## Operating Rules

- [ ] Work in a clean branch or isolated worktree before deleting production code.
- [ ] Add or update a failing guard test before each deletion wave.
- [ ] Keep `reference/openconnect-upstream/` as read-only behavior reference material.
- [ ] Do not add a hidden OpenConnect fallback.
- [ ] Do not execute downloaded CSD/host-scan code.
- [ ] Do not commit raw live logs, raw packet captures, cookies, SAML data, challenge responses, or packet payloads.

## Native-Only Config Surface

- [ ] `src/core/config/config.hpp` has no `openconnect_runtime` field.
- [ ] `src/core/config/config_api.cpp` rejects non-native `vpn_engine` values.
- [ ] `src/core/config/config_persistence.cpp` migrates older `legacy_openconnect` configs to native-only state.
- [ ] `src/core/config/config_show.cpp` no longer prints OpenConnect runtime status.
- [ ] `src/core/config/config_set_value.cpp` no longer offers `legacy_openconnect`.
- [ ] `src/core/use_cases/config_use_cases.cpp` emits native-only config JSON.
- [ ] `src/core/app_api/desktop_status_presenter.cpp` emits native-only runtime fields.
- [ ] `webui/src/stores/config.ts` exposes only native engine state.
- [ ] `webui/src/pages/SettingsPage.vue` has no OpenConnect runtime selector.

## Connection Entrypoints

- [ ] `src/core/app_api/desktop_vpn_actions.cpp` rejects non-native config values before connection start.
- [ ] `src/core/app_api/desktop_vpn_actions.cpp` initializes `TunnelController` for all supported starts.
- [ ] `src/core/rpc/vpn_actions.cpp` uses `TunnelController` only.
- [ ] `src/core/vpn/vpn.cpp` does not route to legacy OpenConnect.
- [ ] `src/core/vpn/vpn_legacy_adapter.cpp` is deleted or absent from production builds.
- [ ] `src/core/vpn/vpn_legacy_adapter.hpp` is deleted or absent from production builds.
- [ ] `src/core/native_orchestration/app_api_native_orchestration.cpp` has no helper-start legacy auth-first path.

## Helper and Supervisor Removal

- [ ] Helper start handlers cannot launch VPN protocol supervisors.
- [ ] Helper cleanup still releases packet leases, adapter state, routes, and DNS.
- [ ] `src/platform/common/vpn_supervisor_process.cpp` is deleted or contains no VPN protocol startup.
- [ ] `src/platform/common/vpn_supervisor_process.hpp` is deleted or contains no VPN protocol payload.
- [ ] No production command line includes `__vpn-supervisor`.
- [ ] No helper IPC request accepts `native_start_mode=password`.
- [ ] No helper IPC request accepts `native_start_mode=auth_session` for VPN startup.
- [ ] `tests/native_helper_session_test.cpp` proves helper is a privileged broker only.
- [ ] `tests/helper_contract_test.cpp` rejects supervisor startup.

## OpenConnect Runtime Removal

- [ ] `src/platform/common/openconnect_process.hpp` is deleted.
- [ ] `src/platform/win32/openconnect_process.cpp` is deleted.
- [ ] `src/platform/darwin/openconnect_process.cpp` is deleted.
- [ ] `src/platform/linux/openconnect_process.cpp` is deleted.
- [ ] `src/platform/common/runtime_discovery.*` no longer searches for OpenConnect binaries.
- [ ] `src/platform/common/runtime_status.cpp` returns native runtime status without OpenConnect diagnostics.
- [ ] `src/core/vpn/openconnect_tunnel_script.*` is deleted.
- [ ] `src/vpn_engine/openconnect/openconnect_log.*` is deleted.
- [ ] `tests/openconnect_log_test.cpp` is deleted or replaced with native metadata tests.
- [ ] `tests/tunnel_script_contract_test.cpp` no longer expects OpenConnect log scraping.

## Packaging and Distribution

- [ ] No packaging script stages `openconnect`, `openconnect.exe`, `libopenconnect`, `libgnutls`, or `vpnc-script`.
- [ ] `scripts/stage-openconnect-runtime-win.ps1` is removed from production packaging.
- [ ] `scripts/stage-openconnect-runtime-mac.sh` is removed from production packaging.
- [ ] Runtime directories contain native assets only.
- [ ] `tests/native_packaging_policy_test.cpp` fails on OpenConnect runtime artifacts.
- [ ] App bundle/resource manifests do not include OpenConnect runtime files.

## Native Protocol Completeness

- [ ] `ProductionProtocolTransport` sends aggregate-auth XML `POST /`.
- [ ] `ProductionProtocolTransport` never sends `/+CSCOE+/logon.html`.
- [ ] `ProductionProtocolTransport` maps success token to `webvpn=`.
- [ ] `ProductionProtocolTransport` sends `CONNECT /CSCOSSLC/tunnel`.
- [ ] `ProductionProtocolTransport` never sends `CONNECT /CSCOT/`.
- [ ] `ProtocolSession` drives CSTP packet forwarding through packet device read/write.
- [ ] `ProtocolSession` services inbound DPD request frames.
- [ ] `ProtocolSession` emits keepalive, DPD, rekey, idle timeout, and session timeout events.
- [ ] CSTP compressed frames produce `cstp_compressed_unsupported`.
- [ ] CSTP terminate/disconnect frames produce `tunnel_disconnected`.

## Auth Continuation

- [ ] Aggregate-auth challenge responses can call `auth_interaction_handler`.
- [ ] Aggregate-auth group selection can call `auth_interaction_handler`.
- [ ] `CoreSessionRunner` stores pending auth interaction metadata.
- [ ] `TunnelController` exposes pending auth interaction to the UI layer.
- [ ] WebUI has a group/challenge dialog that submits values without persisting them.
- [ ] Challenge values are never written to logs, config, session JSON, or handoff docs.
- [ ] Group options are redacted if they include token-like values.
- [ ] Timeout or user cancel returns a stable `auth_challenge_required`, `auth_group_required`, or `user_cancelled` code.

## CSD and Host-Scan

- [ ] Host-scan metadata parser extracts ticket/token/base URI/wait URI.
- [ ] Default behavior is `csd_required_unsupported` or a clearly named reserved-wrapper error.
- [ ] `--csd-wrapper` is either rejected or implemented as a local allowlisted path.
- [ ] No gateway-downloaded executable or script is run by default.
- [ ] CSD ticket/token values are redacted in errors.
- [ ] CSD ticket/token values are redacted in logs.
- [ ] CSD ticket/token values are redacted in persisted session JSON.

## DTLS

- [ ] `disable_dtls=true` skips DTLS capability evaluation that would imply setup.
- [ ] `disable_dtls=false` does not claim production DTLS unless a real backend exists.
- [ ] DTLS metadata from CSTP response is parsed.
- [ ] DTLS unavailable state is non-fatal and leaves CSTP running.
- [ ] UI copy describes native DTLS honestly.
- [ ] Tests prove no `X-DTLS-*` request headers are sent before a real implementation exists.

## Platform Networking

- [ ] Windows native path applies gateway IPv4 address and netmask-derived prefix.
- [ ] Windows native path applies DNS servers.
- [ ] Windows native path applies split include routes.
- [ ] Windows native path applies split exclude routes.
- [ ] Windows native path protects VPN gateway endpoint route.
- [ ] macOS native path applies utun address and MTU.
- [ ] macOS native path applies DNS resolver configuration.
- [ ] macOS native path applies split include and split exclude routes.
- [ ] Linux native path is either implemented or returns explicit `native_transport_unavailable` without fallback.

## Runtime and Status

- [ ] Native runtime status does not probe OpenConnect.
- [ ] Native session state has no OpenConnect PID field.
- [ ] Runtime status does not call `find_openconnect_pid` for native.
- [ ] `status.get` reports native phase from `TunnelController`.
- [ ] Failed native connect clears active connection attempt state.
- [ ] User disconnect prevents auto-reconnect.
- [ ] Core crash cleanup releases helper-owned network resources.

## Docs

- [ ] `docs/ARCHITECTURE_TARGET.md` states native-only production architecture.
- [ ] `docs/architecture/00-constitution.md` no longer lists OpenConnect as a current production core duty.
- [ ] `docs/architecture/10-requirements.md` no longer describes OpenConnect launch as a requirement.
- [ ] `docs/architecture/new_start_point.md` no longer describes current startup as OpenConnect/supervisor.
- [ ] `docs/code_guide.md` no longer shows OpenConnect as the active connection flow.
- [ ] `docs/architecture/native-anyconnect-protocol-requirements.md` distinguishes behavior reference from production runtime.
- [ ] Old diagrams that still describe supervisor/OpenConnect startup are moved to `docs/archive/2026-06/`.
- [ ] `docs/architecture/guardrail_allowlist.yml` does not allow production OpenConnect/supervisor drift.

## Contract and UI

- [ ] `contracts/system.contract.json` has no legacy engine enum.
- [ ] Generated contract files have no `legacy_openconnect`.
- [ ] Generated contract files have no `openconnect_runtime`.
- [ ] WebUI typecheck passes.
- [ ] WebUI settings page has no OpenConnect runtime warning.
- [ ] WebUI error presentation still covers native errors: `auth_protocol_mismatch`, `auth_rejected`, `auth_challenge_required`, `auth_group_required`, `auth_expired`, `csd_required_unsupported`, `dtls_unavailable`, `session_timeout`, `idle_timeout`, `unsupported_extra_args`.

## Tests to Run

- [ ] `cmake --build build-windows\cpp --target native_only_cutover_contract_test`
- [ ] `.\build-windows\cpp\native_only_cutover_contract_test.exe`
- [ ] `cmake --build build-windows\cpp --target native_protocol_session_test native_production_transport_test native_engine_contract_test native_dtls_transport_test native_engine_config_mapper_test`
- [ ] `.\build-windows\cpp\native_protocol_session_test.exe`
- [ ] `.\build-windows\cpp\native_production_transport_test.exe`
- [ ] `.\build-windows\cpp\native_engine_contract_test.exe`
- [ ] `.\build-windows\cpp\native_dtls_transport_test.exe`
- [ ] `.\build-windows\cpp\native_engine_config_mapper_test.exe`
- [ ] `cmake --build build-windows\cpp --target tunnel_controller_integration_test runtime_status_native_test native_packaging_policy_test contract_manifest_test`
- [ ] `.\build-windows\cpp\tunnel_controller_integration_test.exe`
- [ ] `.\build-windows\cpp\runtime_status_native_test.exe`
- [ ] `.\build-windows\cpp\native_packaging_policy_test.exe`
- [ ] `.\build-windows\cpp\contract_manifest_test.exe`
- [ ] `python scripts\generate_contracts.py --check`
- [ ] `cd webui; pnpm exec vue-tsc -b; cd ..`
- [ ] `git diff --check`

## Banned Production Tokens

Run this scan before completion:

```powershell
rg -n "legacy_openconnect|__vpn-supervisor|spawn_openconnect_process|openconnect_process|configure_from_openconnect_log|openconnect_runtime|webvpn_session=" src webui contracts scripts package.json
```

- [ ] No production hits for `legacy_openconnect`.
- [ ] No production hits for `__vpn-supervisor`.
- [ ] No production hits for `spawn_openconnect_process`.
- [ ] No production hits for `openconnect_process`.
- [ ] No production hits for `configure_from_openconnect_log`.
- [ ] No production hits for `openconnect_runtime`.
- [ ] No production hits for `webvpn_session=`.

## Allowed Reference Tokens

These locations may still contain OpenConnect or legacy words:

- [ ] `reference/openconnect-upstream/` as clean-room behavioral reference.
- [ ] `docs/archive/` as historical record.
- [ ] This checklist.
- [ ] The paired implementation plan.
- [ ] Tests that intentionally assert banned-token scanners fail on seeded fixture text.

## Live Validation

- [ ] Create a dated handoff from `docs/handoffs/native-anyconnect-v2-live-validation-template.md`.
- [ ] Confirm no OpenConnect process is running before connect.
- [ ] Confirm no `__vpn-supervisor` process is running before connect.
- [ ] Confirm runtime status reports native-only.
- [ ] Validate P0 auth/CSTP.
- [ ] Validate P1 DNS/routes/liveness.
- [ ] Validate P2 challenge/group/CSD/DTLS fallback/reconnect.
- [ ] Validate P3 native-only process/package evidence.
- [ ] Redact password values.
- [ ] Redact `webvpn=` values.
- [ ] Redact `<session-token>` values.
- [ ] Redact opaque values.
- [ ] Redact SAML values.
- [ ] Redact challenge responses.
- [ ] Redact cookies and cookie headers.
- [ ] Redact packet payloads.

## Completion Criteria

- [ ] All planned deletion tasks have been completed or explicitly converted into native-only modules.
- [ ] All banned production scans pass.
- [ ] Native smoke tests pass.
- [ ] Contract generation check passes.
- [ ] WebUI typecheck passes.
- [ ] Packaging policy rejects OpenConnect artifacts.
- [ ] Live ECNU evidence is attached or the release remains marked as not live-validated.
- [ ] The final implementation summary lists every retained reference to OpenConnect or supervisor and why it is allowed.
