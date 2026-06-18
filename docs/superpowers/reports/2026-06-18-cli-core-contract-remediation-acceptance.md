# CLI/Core Contract Remediation Acceptance Report

Date: 2026-06-18
Branch: codex/cli-core-ui-contract-refactor
Worktree: D:\Development\Projects\cpp\ECNU-VPN\.worktrees\cli-core-ui-contract-refactor-docs
Implementation commit: aeab90fc9f5ea0ebea7523e29138de06493e3bca
Native-only baseline merged: 4be5cbf

## Scope Accepted

- Added generated action ownership metadata with `core_rpc`, `desktop_host_adapter`, `renderer_only`, and `compat_alias` owners.
- Made `key.status` the canonical credential status action and `config.getKey` a generated compatibility alias to `key.status`.
- Added canonical `logs.clear` contract metadata, generated error code metadata, and a shared core RPC backend used by the desktop log action adapter.
- Rejected duplicate RPC handler registration in `AppRpcDispatcher` and removed `ConfigActions` ownership of `routes.*`; route actions now own route RPC behavior.
- Split `exv-cli` away from the full `exv-core` target and limited it to CLI formatting, IPC transport, core resolver client, neutral lifecycle/runtime helpers, and platform utility sources.

## Verification

### Contract Generation And WebUI Host Contract

Commands:

```powershell
python scripts\generate_contracts.py --check
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
```

Result: exit 0.

Key output:

```text
contract generated files are up to date
tests 12
pass 12
fail 0
```

### Focused CLI/Core Targets

Command:

```powershell
cmake --build build-windows\cpp --target contract_manifest_test app_api_rpc_dispatcher_test app_api_status_contract_test config_actions_test route_actions_test vpn_actions_test feedback_test cli_commands_test core_resolver_test core_architecture_contract_test exv-cli
build-windows\cpp\contract_manifest_test.exe
build-windows\cpp\app_api_rpc_dispatcher_test.exe
build-windows\cpp\app_api_status_contract_test.exe
build-windows\cpp\config_actions_test.exe
build-windows\cpp\route_actions_test.exe
build-windows\cpp\vpn_actions_test.exe
build-windows\cpp\feedback_test.exe
build-windows\cpp\cli_commands_test.exe
build-windows\cpp\core_resolver_test.exe
build-windows\cpp\core_architecture_contract_test.exe
```

Result: exit 0.

Key output:

```text
contract_manifest_test: all assertions passed
app_api_rpc_dispatcher_test: all assertions passed
config_actions_test: all assertions passed
route_actions_test: all assertions passed
vpn_actions_test: all assertions passed
feedback_test: all assertions passed
cli_commands_test: all assertions passed
All core resolver tests passed.
core_architecture_contract_test: all assertions passed
```

### Release Blocking

Command:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\run-tests.ps1 -Preset windows-release -Label release-blocking
```

Result: exit 0.

Key output:

```text
100% tests passed, 0 tests failed out of 71
release-blocking = 22.85 sec*proc (71 tests)
All tests passed!
```

### Diff Hygiene

Command:

```powershell
git diff --check
```

Result: exit 0; no whitespace errors. Git emitted expected line-ending normalization warnings for the Windows checkout.

## Acceptance Decision

Accepted for merge into `codex/ui-framework-webview-shell` as the canonical CLI/core contract remediation baseline.
