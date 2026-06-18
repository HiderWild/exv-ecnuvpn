# Native-Only Audit Remediation Acceptance Report

Date: 2026-06-18
Branch: codex/native-only-cutover
Worktree: D:\Development\Projects\cpp\ECNU-VPN\.worktrees\native-only-cutover
Implementation commit: a71d7d4591dace6dd2d9d580d3e0449c4902a8f0

## Scope Accepted

- Removed active legacy OpenConnect build and packaging references from the native-only cutover path and tightened guardrails around reintroduction.
- Added public absence coverage for retired runtime/config/UI field names: `openconnectBinary`, `openconnectPath`, `openconnectArgs`, `legacyTunnelScript`, and `legacyAdapter`.
- Completed native aggregate-auth coverage for follow-up group selection, structured challenge parsing, and safe SAML-required diagnostics.
- Made DTLS status explicit for the native-only cutover, including TLS fallback semantics when the gateway advertises DTLS but native DTLS transport is unavailable.
- Propagated structured native errors through the tunnel controller bridge with public codes, remediation hints, recoverability, and log/message redaction.

## Verification

### Release Blocking

Command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\run-tests.ps1 -Preset windows-release -Label release-blocking
```

Result: exit 0.

Key output:

```text
100% tests passed, 0 tests failed out of 71
release-blocking = 22.54 sec*proc (71 tests)
All tests passed!
```

### Focused Native Protocol Gate

Command:

```powershell
cmake --build build-windows\cpp --target native_aggregate_auth_test native_dtls_transport_test native_production_transport_test native_protocol_session_test native_packaging_policy_test
build-windows\cpp\native_aggregate_auth_test.exe
build-windows\cpp\native_dtls_transport_test.exe
build-windows\cpp\native_production_transport_test.exe
build-windows\cpp\native_protocol_session_test.exe
build-windows\cpp\native_packaging_policy_test.exe
```

Result: exit 0.

Key output:

```text
ninja: no work to do.
native_aggregate_auth_test: all assertions passed
native_dtls_transport_test: all assertions passed
focused-native exit=0
```

The remaining focused executables are silent on success and returned exit 0.

### Guardrails And UI Contract

Commands:

```powershell
cmake --build build-windows\cpp --target app_api_status_contract_test runtime_status_native_test native_packaging_policy_test
build-windows\cpp\app_api_status_contract_test.exe
build-windows\cpp\runtime_status_native_test.exe
build-windows\cpp\native_packaging_policy_test.exe
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
powershell -ExecutionPolicy Bypass -File scripts\architecture-guardrails.ps1
bash scripts/architecture-guardrails.sh
```

Result: exit 0 for all commands.

Key output:

```text
host/__tests__/desktop-contract-generated.test.ts: 11 tests passed
Architecture guardrails: all checks passed
```

### Diff Hygiene

Command:

```powershell
git diff --check
```

Result: exit 0; no whitespace errors. Git emitted expected line-ending normalization warnings for existing Windows checkout behavior.

## Acceptance Decision

Accepted for merge into `codex/cli-core-ui-contract-refactor` as the native-only remediation baseline.
