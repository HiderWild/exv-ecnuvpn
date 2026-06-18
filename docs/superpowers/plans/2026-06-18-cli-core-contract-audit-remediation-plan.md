# CLI Core Contract Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve the June 18 audit findings that belong to the shared core/CLI contract layer before those fixes are merged forward into the WebView shell branch.

**Architecture:** `codex/cli-core-ui-contract-refactor` is the source branch for canonical App API, RPC action ownership, CLI boundary, and generated contract fixes. Implement and test these fixes here first. After green verification, merge this branch into `codex/ui-framework-webview-shell`, then implement UI-specific follow-up work there.

**Tech Stack:** C++20, CMake/Ninja, nlohmann/json, generated C++/TypeScript contract artifacts, App API dispatcher code, core RPC actions, CLI frontend process, PowerShell release test scripts.

---

## Branch Placement

- Worktree: `D:\Development\Projects\cpp\ECNU-VPN\.worktrees\cli-core-ui-contract-refactor`
- Branch: `codex/cli-core-ui-contract-refactor`
- Downstream consumer: `codex/ui-framework-webview-shell`
- Do not solve WebView host parity or native-only OpenConnect deletion in this branch except where a generated contract shape is needed by downstream branches.

## Audit Findings Assigned Here

1. App API and core RPC dispatchers still duplicate behavior instead of proving one canonical action owner.
2. Route/config/vpn actions are split across similarly named files with inconsistent ownership, increasing drift risk.
3. `vpn.connect`, `vpn.disconnect`, and `status.get` have different response shapes depending on whether the call enters through desktop App API or core RPC.
4. `config.getKey` and `key.status` naming remains ambiguous; compatibility aliases need explicit generated-contract tests or deletion.
5. `exv-cli` is split as a binary but still links too much core implementation and pulls UI/core resolver concepts through `src/platform/common/core_resolver_platform_deps.cpp`.
6. The logs clear action needs one canonical contract/backend implementation before UI can expose it safely.

## Phase 1: Make Action Ownership Explicit

**Files:**
- Modify: `contracts/system.contract.json`
- Modify: `scripts/generate_contracts.py`
- Modify: `src/contracts/generated/system_contract.hpp`
- Modify: `contracts/generated/system_contract_snapshot.json`
- Modify: `webui/host/shared/generated/system-contract.ts`
- Modify: `webui/desktop/shared/generated/system-contract.ts`
- Modify: `tests/contract_manifest_test.cpp`
- Modify: `webui/host/__tests__/desktop-contract-generated.test.ts`

- [ ] **Step 1: Add generated ownership metadata**

Extend the manifest and generator so every public action has exactly one owner:

```text
core_rpc
desktop_host_adapter
renderer_only
compat_alias
```

Add tests that fail if any action appears in more than one owner set, or if a `compat_alias` points to a missing canonical action.

- [ ] **Step 2: Encode compatibility aliases**

Decide the canonical key action. Preferred outcome: `key.status` is canonical and `config.getKey` is a tested compatibility alias until the compatibility window closes.

Required test assertions:

```cpp
expect(is_core_rpc_action("key.status"), "key.status must be core owned");
expect(is_compat_alias("config.getKey"), "config.getKey must be an alias, not a second owner");
expect(canonical_action_for("config.getKey") == "key.status", "config.getKey must route to key.status");
```

- [ ] **Step 3: Verify**

Run:

```powershell
python scripts\generate_contracts.py
cmake --build build-windows\cpp --target contract_manifest_test
build-windows\cpp\contract_manifest_test.exe
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
```

Expected result: generated C++ and TypeScript metadata identify exactly one owner for each action.

## Phase 2: Unify Desktop App API And Core RPC Dispatch

**Files:**
- Modify: `src/core/app_api/app_api.cpp`
- Modify: `src/core/app_api/desktop_action_registry.cpp`
- Modify: `src/core/app_api/desktop_action_registry.hpp`
- Modify: `src/core/rpc/core_api_setup.cpp`
- Modify: `src/core/rpc/core_api_setup.hpp`
- Modify: `src/core/rpc/desktop_rpc_adapter.cpp`
- Modify: `src/core/rpc/desktop_rpc_adapter.hpp`
- Modify: `src/core/core_process.cpp`
- Modify: `tests/app_api_rpc_dispatcher_test.cpp`
- Modify: `tests/app_api_status_contract_test.cpp`

- [ ] **Step 1: Add dispatcher equivalence tests**

For each shared action, call through both the desktop App API adapter and core RPC setup and compare normalized JSON output:

```text
status.get
vpn.connect
vpn.disconnect
routes.list
config.getSettings
key.status
logs.clear
```

The tests must fail if either path returns different `ok`, `error.code`, `data.status`, or `data.version` fields for the same mocked backend.

- [ ] **Step 2: Keep only one implementation per action**

Make desktop actions thin adapters over the core-owned dispatcher for shared actions. Keep host-only actions in `desktop_action_registry` only when the manifest owner is `desktop_host_adapter`.

- [ ] **Step 3: Verify**

Run:

```powershell
cmake --build build-windows\cpp --target app_api_rpc_dispatcher_test app_api_status_contract_test
build-windows\cpp\app_api_rpc_dispatcher_test.exe
build-windows\cpp\app_api_status_contract_test.exe
```

Expected result: shared actions have one backend behavior regardless of UI, CLI, or core RPC entrypoint.

## Phase 3: Consolidate Config, Route, VPN, And Logs Actions

**Files:**
- Modify: `src/core/rpc/config_actions.cpp`
- Modify: `src/core/rpc/config_actions.hpp`
- Modify: `src/core/rpc/route_actions.cpp`
- Modify: `src/core/rpc/route_actions.hpp`
- Modify: `src/core/rpc/vpn_actions.cpp`
- Modify: `src/core/rpc/vpn_actions.hpp`
- Modify: `src/core/app_api/desktop_vpn_actions.cpp`
- Modify: `src/core/app_api/desktop_system_actions.cpp`
- Modify: `tests/config_actions_test.cpp`
- Modify: `tests/route_actions_test.cpp`
- Modify: `tests/vpn_actions_test.cpp`
- Modify: `tests/feedback_test.cpp`

- [ ] **Step 1: Add drift tests for duplicated action names**

Create or extend tests so the source tree fails if these action strings are implemented in both desktop and core files:

```text
routes.list
routes.add
routes.delete
config.getSettings
config.setValue
vpn.connect
vpn.disconnect
status.get
logs.clear
```

- [ ] **Step 2: Move shared logic behind core RPC handlers**

For every shared action, desktop App API code should validate host-only envelope fields and call the core handler. Business behavior belongs in core use cases or core RPC actions.

- [ ] **Step 3: Implement canonical `logs.clear` backend**

Make `logs.clear` a generated core action with a stable result shape:

```json
{
  "ok": true,
  "cleared": true,
  "logDirectory": "<redacted-or-public-path>",
  "filesRemoved": 0
}
```

Failures must use generated error codes and avoid leaking private paths when the caller is renderer-facing.

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build build-windows\cpp --target config_actions_test route_actions_test vpn_actions_test feedback_test
build-windows\cpp\config_actions_test.exe
build-windows\cpp\route_actions_test.exe
build-windows\cpp\vpn_actions_test.exe
build-windows\cpp\feedback_test.exe
```

Expected result: duplicated behavior is removed or covered by explicit adapter tests.

## Phase 4: Tighten The CLI Split

**Files:**
- Modify: `CMakeLists.txt`
- Modify: `src/cli/cli_main.cpp`
- Modify: `src/cli/*`
- Modify: `src/platform/common/core_resolver_platform_deps.cpp`
- Modify: `src/platform/common/core_resolver.*`
- Modify: `tests/cli_commands_test.cpp`
- Modify: `tests/core_resolver_test.cpp`
- Modify: `tests/core_architecture_contract_test.cpp`

- [ ] **Step 1: Add link-boundary tests**

Extend architecture tests so `exv-cli` cannot directly link native engine, UI shell, or desktop App API implementation objects. It may link:

```text
contract/generated metadata
CLI argument parsing and formatting
neutral IPC client
core resolver client
small platform IPC transport helpers
```

- [ ] **Step 2: Split resolver platform dependencies**

Move process discovery and IPC path resolution into a neutral client-side library. Keep server-side core startup internals outside `exv-cli`.

- [ ] **Step 3: Keep CLI behavior stable**

Update CLI tests for `connect`, `disconnect`, `status`, config commands, and destructive confirmations. The CLI should call generated canonical actions and print stable terminal output.

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build build-windows\cpp --target cli_commands_test core_resolver_test core_architecture_contract_test
build-windows\cpp\cli_commands_test.exe
build-windows\cpp\core_resolver_test.exe
build-windows\cpp\core_architecture_contract_test.exe
```

Expected result: `exv-cli` is a thin frontend over IPC and generated contracts, not a second core binary.

## Phase 5: Merge Forward To WebView Shell

- [ ] **Step 1: Commit the green CLI/core remediation**

Commit this branch after Phases 1 through 4 pass. Include the command outputs in a report under `docs/superpowers/reports/`.

- [ ] **Step 2: Merge into downstream branch**

From the downstream UI worktree:

```powershell
git checkout codex/ui-framework-webview-shell
git merge codex/cli-core-ui-contract-refactor
```

Resolve conflicts by preserving generated contract ownership and canonical core action behavior.

- [ ] **Step 3: Rerun the downstream contract gate before UI-specific fixes**

Run:

```powershell
cmake --build build-windows\cpp --target contract_manifest_test app_api_rpc_dispatcher_test app_api_status_contract_test
build-windows\cpp\contract_manifest_test.exe
build-windows\cpp\app_api_rpc_dispatcher_test.exe
build-windows\cpp\app_api_status_contract_test.exe
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
```

Expected result: the downstream branch starts its UI-specific remediation from the same core/contract behavior.
