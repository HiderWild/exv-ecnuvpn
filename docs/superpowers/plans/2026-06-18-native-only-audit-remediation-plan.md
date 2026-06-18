# Native-Only Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Finish the native-only cutover by removing active OpenConnect production paths, replacing legacy config/runtime/UI assumptions, and closing the remaining native AnyConnect protocol and error propagation gaps found in the June 18 audit.

**Architecture:** Keep `codex/native-only-cutover` focused on native-engine completion and legacy path deletion. Core/UI/CLI contract shape fixes belong on `codex/cli-core-ui-contract-refactor` and `codex/ui-framework-webview-shell`; this branch only consumes those merges after its own native cutover tests are green.

**Tech Stack:** C++20, CMake/Ninja, generated contract artifacts, native AnyConnect protocol code under `src/vpn_engine`, tunnel controller integration tests, existing WebUI host contract tests for packaging guardrails.

---

## Branch Placement

- Worktree: `D:\Development\Projects\cpp\ECNU-VPN\.worktrees\native-only-cutover`
- Branch: `codex/native-only-cutover`
- This document covers only issues that should be fixed on the native-only branch.
- Do not stage or rewrite work from the dirty `codex/ui-framework-webview-shell` worktree.
- After this branch is green, merge in the current `codex/cli-core-ui-contract-refactor` and `codex/ui-framework-webview-shell` heads only if the native-only conflict set is understood and tests are rerun.

## Audit Findings Assigned Here

1. OpenConnect sources and tests are still referenced by production CMake and validation scripts.
2. Legacy OpenConnect config/runtime fields still leak through config APIs, runtime status, WebUI settings, and manual scripts.
3. The guardrail allowlist still tolerates legacy paths too broadly, making reintroduction hard to detect.
4. Native AnyConnect v2 support is materially improved but not complete enough to declare the native-only cutover finished: aggregate auth group selection, challenge parsing, SAML public diagnostics, DTLS status/fallback semantics, and native error propagation need explicit tests.
5. Native production transport and tunnel controller tests need to prove failures surface through the public error contract rather than generic fallback messages.

## Phase 1: Remove Active OpenConnect Build Inputs

**Files:**
- Modify: `CMakeLists.txt`
- Modify: `tests/native_packaging_policy_test.cpp`
- Modify: `docs/architecture/guardrail_allowlist.yml`
- Modify: `scripts/architecture-guardrails.ps1`
- Modify: `scripts/architecture-guardrails.sh`

- [ ] **Step 1: Add failing policy assertions**

Extend `tests/native_packaging_policy_test.cpp` so it fails if active targets or production source lists contain:

```text
openconnect_process
openconnect_log
vpn_legacy_adapter
openconnect_tunnel_script
```

The test must allow historical docs only when the path is listed under a narrowly named historical exception in `docs/architecture/guardrail_allowlist.yml`.

- [ ] **Step 2: Remove production CMake references**

Delete OpenConnect entries from executable/library source lists and release-blocking tests. Keep any historical compatibility documentation outside active build targets.

- [ ] **Step 3: Tighten allowlist semantics**

Rename broad allowlist entries into explicit historical buckets, for example `legacy_openconnect_docs_only`, and make the guardrail scripts reject legacy paths in `src/`, `tests/`, `scripts/`, `webui/src`, and active CMake target definitions.

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build build-windows\cpp --target native_packaging_policy_test
build-windows\cpp\native_packaging_policy_test.exe
powershell -ExecutionPolicy Bypass -File scripts\architecture-guardrails.ps1
```

Expected result: policy test and guardrail script fail before deletion, then pass after the build inputs and allowlist are narrowed.

## Phase 2: Delete Legacy Config, Runtime, And UI Exposure

**Files:**
- Modify: `src/core/config/config.hpp`
- Modify: `src/core/rpc/config_api.cpp`
- Modify: `src/core/rpc/config_show.cpp`
- Modify: `src/core/rpc/config_set_value.cpp`
- Modify: `src/platform/common/runtime_status.cpp`
- Modify: `webui/src/stores/config.ts`
- Modify: `webui/src/pages/SettingsPage.vue`
- Modify: `tests/app_api_status_contract_test.cpp`
- Modify: `tests/runtime_status_native_test.cpp`
- Modify: `webui/host/__tests__/desktop-contract-generated.test.ts`

- [ ] **Step 1: Add failing absence tests**

Add C++ and TypeScript assertions that these legacy field names are absent from public status/config payloads and generated desktop contract snapshots:

```text
openconnectBinary
openconnectPath
openconnectArgs
legacyTunnelScript
legacyAdapter
```

- [ ] **Step 2: Replace config fields with native engine configuration**

Move any still-needed user settings to native-engine names under the existing config schema. Do not retain compatibility aliases unless a generated contract alias test proves the alias is intentionally public.

- [ ] **Step 3: Remove UI controls that configure legacy transport**

Delete renderer settings that expose OpenConnect binary/script knobs. Replace them only with native-engine settings that map to `src/vpn_engine` configuration.

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build build-windows\cpp --target app_api_status_contract_test runtime_status_native_test
build-windows\cpp\app_api_status_contract_test.exe
build-windows\cpp\runtime_status_native_test.exe
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
```

Expected result: no public payload or renderer settings path contains legacy OpenConnect configuration names.

## Phase 3: Complete Native Aggregate Auth And SAML Diagnostics

**Files:**
- Modify: `src/vpn_engine/protocol/aggregate_auth.hpp`
- Modify: `src/vpn_engine/protocol/aggregate_auth.cpp`
- Modify: `src/vpn_engine/protocol/native_auth_session_json.cpp`
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `src/vpn_engine/protocol/session.hpp`
- Modify: `tests/native_aggregate_auth_test.cpp`
- Modify: `tests/native_auth_session_json_test.cpp`
- Modify: `tests/native_protocol_session_test.cpp`

- [ ] **Step 1: Add coverage for follow-up auth groups**

Add tests where the fake AnyConnect server returns multiple groups, a follow-up selection form, and a second challenge. The client must preserve selected group identity across the follow-up exchange.

- [ ] **Step 2: Replace brittle challenge heuristics with structured parsing**

Make challenge handling depend on parsed aggregate-auth form fields instead of substring-only prompts. Include tests for password-only, password-plus-token, and informational challenge prompts.

- [ ] **Step 3: Add public SAML diagnostic mapping**

Expose SAML-required and unsupported-browser cases through stable native error/diagnostic codes. Keep private URLs, cookies, and tokens out of logs.

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build build-windows\cpp --target native_aggregate_auth_test native_auth_session_json_test native_protocol_session_test
build-windows\cpp\native_aggregate_auth_test.exe
build-windows\cpp\native_auth_session_json_test.exe
build-windows\cpp\native_protocol_session_test.exe
```

Expected result: aggregate auth and SAML diagnostics are covered by structured protocol tests, not manual-only checklist entries.

## Phase 4: Settle DTLS Scaffold And Native Fallback Semantics

**Files:**
- Modify: `src/vpn_engine/protocol/dtls_transport.hpp`
- Modify: `src/vpn_engine/protocol/dtls_transport.cpp`
- Modify: `src/vpn_engine/protocol/production_transport.cpp`
- Modify: `src/vpn_engine/protocol/production_transport.hpp`
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `tests/native_dtls_transport_test.cpp`
- Modify: `tests/native_production_transport_test.cpp`

- [ ] **Step 1: Add explicit DTLS state tests**

Test all supported states:

```text
disabled
attempted_and_connected
attempted_and_fell_back_to_tls
attempted_and_failed_without_tls_fallback
```

- [ ] **Step 2: Decide the cutover requirement**

If full DTLS is required for native-only release, implement the transport. If TLS-only CSTP is acceptable for the first native-only cutover, make that a public policy in status payloads and tests. Do not leave DTLS as an ambiguous stub.

- [ ] **Step 3: Verify**

Run:

```powershell
cmake --build build-windows\cpp --target native_dtls_transport_test native_production_transport_test
build-windows\cpp\native_dtls_transport_test.exe
build-windows\cpp\native_production_transport_test.exe
```

Expected result: native status clearly reports whether DTLS is active, unavailable by policy, or failed with TLS fallback.

## Phase 5: Propagate Native Errors Through Tunnel Controller

**Files:**
- Modify: `src/vpn_engine/native_error_contract.hpp`
- Modify: `src/core/tunnel_controller/core_error_mapper.cpp`
- Modify: `src/core/tunnel_controller/core_error_mapper.hpp`
- Modify: `src/core/tunnel_controller/engine_event_bridge.cpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_connect.cpp`
- Modify: `tests/core_error_mapper_test.cpp`
- Modify: `tests/engine_event_bridge_test.cpp`
- Modify: `tests/core_session_runner_test.cpp`
- Modify: `tests/tunnel_controller_integration_test.cpp`

- [ ] **Step 1: Add failing end-to-end native error tests**

Cover certificate failure, auth form failure, SAML-required, DTLS fallback, DNS/connect timeout, and protocol parse failure. Each test must assert the public error code, severity, remediation hint, and absence of secrets in message text.

- [ ] **Step 2: Map native engine events without lossy generic fallback**

Ensure `engine_event_bridge` preserves structured native error fields and `core_error_mapper` only uses generic errors for truly unknown cases.

- [ ] **Step 3: Verify**

Run:

```powershell
cmake --build build-windows\cpp --target core_error_mapper_test engine_event_bridge_test core_session_runner_test tunnel_controller_integration_test
build-windows\cpp\core_error_mapper_test.exe
build-windows\cpp\engine_event_bridge_test.exe
build-windows\cpp\core_session_runner_test.exe
build-windows\cpp\tunnel_controller_integration_test.exe
```

Expected result: native connection failures reach App API and UI-facing status as stable public errors.

## Phase 6: Final Branch Gate

- [ ] **Step 1: Run release-blocking native tests**

Run:

```powershell
./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking
```

- [ ] **Step 2: Run native protocol focus set**

Run:

```powershell
cmake --build build-windows\cpp --target native_aggregate_auth_test native_dtls_transport_test native_production_transport_test native_protocol_session_test native_packaging_policy_test
build-windows\cpp\native_aggregate_auth_test.exe
build-windows\cpp\native_dtls_transport_test.exe
build-windows\cpp\native_production_transport_test.exe
build-windows\cpp\native_protocol_session_test.exe
build-windows\cpp\native_packaging_policy_test.exe
```

- [ ] **Step 3: Record acceptance**

Add a short report under `docs/superpowers/reports/` with the exact commit, commands, and outputs. Only mark the native-only cutover done after the report includes green release-blocking and focused native protocol evidence.
