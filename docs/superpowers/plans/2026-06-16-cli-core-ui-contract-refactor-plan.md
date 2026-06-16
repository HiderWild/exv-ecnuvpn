# CLI Core UI Contract Refactor Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `exv-ui` and `exv-cli` modular frontends over one core-owned backend contract while keeping the native release-blocking test suite green after every phase.

**Architecture:** The refactor proceeds from contract guardrails to runtime ownership. First expand the generated contract and tests, then add core identity/lifecycle primitives, shared core discovery, core-owned config/key/maintenance actions, a thin IPC-only CLI executable, and UI parity controls. Each task must leave `exv` able to run as core and answer `status.get`.

**Tech Stack:** C++20, CMake/Ninja, nlohmann/json, generated C++/TypeScript contract artifacts, Vue 3/Pinia/Vite for renderer UI, Windows named pipes/Unix sockets for IPC.

---

## Execution Rules

- Work from `D:\Development\Projects\cpp\ECNU-VPN\.worktrees\cli-core-ui-contract-refactor`.
- Branch: `codex/cli-core-ui-contract-refactor`.
- Default worker and reviewer model: GPT-5.4 with XHIGH reasoning.
- If the same subtask is rejected twice by spec or quality review for the same unresolved requirement, dispatch one GPT-5.5 XHIGH fixer for that subtask only. New subtasks return to GPT-5.4 XHIGH.
- Use TDD for behavior changes: write a failing test, run it to confirm the expected failure, implement the minimum code, rerun the focused test, then rerun the phase test set.
- Commit after each task or narrow phase. Do not combine unrelated phases.
- After every implementer report, run spec-compliance review first. Run code-quality review only after spec compliance passes.

## Baseline

Already verified on this branch:

```powershell
cd D:\Development\Projects\cpp\ECNU-VPN\.worktrees\cli-core-ui-contract-refactor\webui
pnpm install
pnpm run build

cd D:\Development\Projects\cpp\ECNU-VPN\.worktrees\cli-core-ui-contract-refactor
./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking
```

Expected baseline result: `100% tests passed, 0 tests failed out of 67`.

## File Structure Map

- Modify `contracts/system.contract.json`: canonical action/error metadata.
- Modify `scripts/generate_contracts.py`: generated C++/TypeScript constants for core actions, destructive actions, standard errors, and protocol/contract versions.
- Modify generated outputs:
  - `src/contracts/generated/system_contract.hpp`
  - `webui/host/shared/generated/system-contract.ts`
  - `webui/desktop/shared/generated/system-contract.ts`
  - `contracts/generated/system_contract_snapshot.json`
- Modify `webui/host/shared/host-contract.ts`: re-export generated action sets and remove duplicate hand-maintained action constants as consumers move to the generated contract.
- Guard `src/core/config/modules/config_contract.cppm` and `src/core/tunnel_controller/modules/tunnel_contract.cppm`: these module mirrors must not drift from generated contract metadata; move them into `scripts/generate_contracts.py` only when the CMake module build remains stable.
- Create `src/core/lifecycle/*`: core identity, versioned IPC paths, lock, registry, and registry risk summarization.
- Create `src/platform/common/core_resolver.*`: shared core executable discovery/startup and broken-communication classification.
- Modify `src/core/core_process.*`, `src/core/pipe_ipc.*`, `src/core/rpc/*`: `core.hello`, maintenance actions, native contract routing, lifecycle startup/shutdown.
- Modify `src/core/use_cases/config_use_cases.*`, `src/core/rpc/config_actions.*`, `src/core/rpc/route_actions.*`: shared config/key/route/import/export actions.
- Create `src/cli/cli_main.cpp` and related thin CLI helpers under `src/cli/`: argument parsing, IPC request construction, terminal formatting, confirmation prompts.
- Modify `src/app/main.cpp`: core-only runtime entry and compatibility shims; user CLI behavior moves to `exv-cli`.
- Modify `CMakeLists.txt`: separate `exv` and `exv-cli` executables, new libraries/tests.
- Modify `src/app/ui_shell/*` and `webui/src/*`: shared resolver usage and UI parity controls.
- Add focused tests under `tests/` and `webui/host/__tests__` or `webui/src/**/__tests__` as needed.

## Task 1: Contract Manifest And Generated Constants

**Files:**
- Modify: `contracts/system.contract.json`
- Modify: `scripts/generate_contracts.py`
- Modify: `tests/contract_manifest_test.cpp`
- Modify: `tests/config_module_smoke_test.cpp`
- Modify: `tests/tunnel_contract_test.cpp`
- Modify: `webui/host/__tests__/desktop-contract-generated.test.ts`
- Modify: `webui/host/shared/host-contract.ts`
- Generated: `src/contracts/generated/system_contract.hpp`
- Generated: `webui/host/shared/generated/system-contract.ts`
- Generated: `webui/desktop/shared/generated/system-contract.ts`
- Generated: `contracts/generated/system_contract_snapshot.json`

- [ ] **Step 1: Write failing contract tests**

Add assertions to `tests/contract_manifest_test.cpp` that require:

```cpp
ok = expect(exv::contracts::generated::is_core_rpc_action("core.hello"),
            "core.hello must be generated as a core RPC action") && ok;
ok = expect(exv::contracts::generated::is_core_rpc_action("config.export"),
            "config.export must be generated as a core RPC action") && ok;
ok = expect(exv::contracts::generated::is_core_rpc_action("maintenance.killStaleCore"),
            "maintenance.killStaleCore must be generated as a core RPC action") && ok;
ok = expect(exv::contracts::generated::is_destructive_core_rpc_action("config.reset"),
            "config.reset must require explicit confirmation") && ok;
ok = expect(exv::contracts::generated::is_destructive_core_rpc_action("key.reset"),
            "key.reset must require explicit confirmation") && ok;
ok = expect(exv::contracts::generated::is_destructive_core_rpc_action("maintenance.killStaleCore"),
            "maintenance.killStaleCore must require explicit confirmation") && ok;
ok = expect(exv::contracts::generated::is_standard_error_code("core_comm_broken"),
            "core_comm_broken must be a generated standard error") && ok;
ok = expect(exv::contracts::generated::IPC_PROTOCOL_MAJOR == 1,
            "IPC protocol major must be generated") && ok;
```

Add TypeScript assertions to `webui/host/__tests__/desktop-contract-generated.test.ts`:

```ts
assert(CORE_RPC_ACTIONS.includes('core.hello'))
assert(CORE_RPC_ACTIONS.includes('config.import'))
assert(CORE_RPC_ACTIONS.includes('maintenance.inspectCore'))
assert(DESTRUCTIVE_CORE_RPC_ACTIONS.includes('key.reset'))
assert(STANDARD_ERROR_CODES.includes('config_import_tampered_or_wrong_password'))
assert.equal(IPC_PROTOCOL_MAJOR, 1)
```

Add drift assertions that `host-contract.ts` no longer defines independent legacy communication action maps once their generated equivalents exist:

```ts
assert(!source.includes("export const CONFIG_ACTIONS = {"))
assert(!source.includes("export const ROUTE_ACTIONS = {"))
assert(CORE_RPC_ACTIONS.includes('routes.list'))
assert(CONFIG_ALIASES['config.get'] === 'config.getSettings')
```

Add focused assertions to `tests/config_module_smoke_test.cpp` and `tests/tunnel_contract_test.cpp` that compare the module mirror counts/names against `exv::contracts::generated::CONFIG_ACTIONS`, `CONFIG_ALIASES`, `TUNNEL_PHASE_CONTRACTS`, and `TUNNEL_EVENTS`. If direct module-to-header comparison is blocked by compiler module limitations, keep the comparison in `tests/contract_manifest_test.cpp` by reading the module files as text and matching every generated value.

- [ ] **Step 2: Verify the tests fail**

Run:

```powershell
cmake --build --preset windows-release --target contract_manifest_test
ctest --test-dir build-windows/cpp -R contract_manifest_test --output-on-failure
cd webui
pnpm run test:contract
```

Expected: both fail because the generated constants do not exist or do not include the new values.

- [ ] **Step 3: Expand the manifest**

In `contracts/system.contract.json`:

- Bump `version` to `2026-06-16.cli-core-ui-contract.v1`.
- Add `ipc_protocol_major: 1`.
- Add `surfaces.core_rpc.actions` with exactly:
  `core.hello`, `status.get`, `vpn.connect`, `vpn.disconnect`, `config.get`,
  `config.saveAuth`, `config.saveSettings`, `config.reset`, `config.import`,
  `config.export`, `key.status`, `key.reset`, `routes.list`, `routes.add`,
  `routes.remove`, `routes.reset`, `logs.list`, `service.status`,
  `service.install`, `service.uninstall`, `runtime.status`, `drivers.status`,
  `drivers.install`, `maintenance.inspectCore`, `maintenance.killStaleCore`.
- Add `surfaces.core_rpc.destructive_actions` with exactly:
  `config.reset`, `key.reset`, `maintenance.killStaleCore`.
- Add `surfaces.core_rpc.error_codes` with the standard errors from the design:
  `confirmation_required`, `invalid_payload`, `invalid_config`,
  `unsupported_contract_version`, `core_comm_broken`, `core_unresponsive`,
  `core_protocol_mismatch`, `core_not_found`, `core_launch_failed`,
  `core_version_probe_failed`, `config_import_format_unsupported`,
  `config_import_auth_failed`, `config_import_tampered_or_wrong_password`,
  `credential_store_unavailable`, `key_missing`, `key_corrupt`.
- Keep existing desktop actions during migration.

- [ ] **Step 4: Update generator validation and outputs**

In `scripts/generate_contracts.py`:

- Validate `ipc_protocol_major` as a positive integer.
- Validate `surfaces.core_rpc.actions`, `destructive_actions`, and `error_codes` with duplicate detection.
- Render C++ constants:
  `IPC_PROTOCOL_MAJOR`, `CORE_RPC_ACTIONS`, `DESTRUCTIVE_CORE_RPC_ACTIONS`, `STANDARD_ERROR_CODES`, `is_core_rpc_action`, `is_destructive_core_rpc_action`, `is_standard_error_code`.
- Render TypeScript constants with the same names.
- Remove duplicate hand-maintained communication action maps from `webui/host/shared/host-contract.ts`; keep domain payload types there, but derive action-name exports from generated constants.
- If safe for the current CMake module build, generate `src/core/config/modules/config_contract.cppm` and `src/core/tunnel_controller/modules/tunnel_contract.cppm` from the manifest. If not, add text-level drift guards in tests so these mirrors must match generated metadata.

- [ ] **Step 5: Regenerate and verify**

Run:

```powershell
python scripts/generate_contracts.py
cmake --build --preset windows-release --target contract_manifest_test
ctest --test-dir build-windows/cpp -R contract_manifest_test --output-on-failure
cd webui
pnpm run test:contract
```

Expected: focused tests pass.

- [ ] **Step 6: Commit**

```powershell
git add contracts/system.contract.json contracts/generated/system_contract_snapshot.json scripts/generate_contracts.py src/contracts/generated/system_contract.hpp webui/host/shared/generated/system-contract.ts webui/desktop/shared/generated/system-contract.ts webui/host/shared/host-contract.ts tests/contract_manifest_test.cpp tests/config_module_smoke_test.cpp tests/tunnel_contract_test.cpp webui/host/__tests__/desktop-contract-generated.test.ts
git commit -m "contract: expand core ipc action manifest"
```

## Task 2: Core Hello And Native Contract Version Response

**Files:**
- Create: `src/core/lifecycle/core_identity.hpp`
- Create: `src/core/lifecycle/core_identity.cpp`
- Create: `src/core/rpc/core_actions.hpp`
- Create: `src/core/rpc/core_actions.cpp`
- Modify: `src/core/rpc/core_api_setup.cpp`
- Modify: `src/core/core_process.cpp`
- Modify: `CMakeLists.txt`
- Create: `tests/core_api/core_hello_actions_test.cpp`
- Modify: `tests/core_process_lifecycle_test.cpp`

- [ ] **Step 1: Write the failing core action test**

Create `tests/core_api/core_hello_actions_test.cpp` with assertions that:

```cpp
auto dispatcher = exv::core_api::create_dispatcher(controller);
auto response = dispatch(*dispatcher, "core.hello", R"({"contract_version":"2026-06-16.cli-core-ui-contract.v1"})");
expect(response.success, "core.hello should succeed for accepted contract version");
auto payload = json::parse(response.payload_json);
expect(payload["ipc_protocol_version"] == "ipc-v1", "hello returns protocol version string");
expect(payload["contract_version"] == std::string(exv::contracts::generated::CONTRACT_VERSION), "hello returns contract version");
expect(payload["app_version"] == ECNUVPN_VERSION, "hello returns app version");
expect(payload.contains("core_instance_id"), "hello returns instance id");
expect(payload.contains("pid"), "hello returns pid");
expect(payload.contains("core_path"), "hello returns core path");
expect(payload.contains("started_at"), "hello returns started_at");
```

Add a second assertion:

```cpp
auto bad = dispatch(*dispatcher, "core.hello", R"({"contract_version":"wrong"})");
expect(!bad.success, "core.hello rejects incompatible contract version");
expect(bad.error_code == "unsupported_contract_version", "mismatch uses unsupported_contract_version");
```

- [ ] **Step 2: Verify the test fails**

Run:

```powershell
cmake --build --preset windows-release --target core_hello_actions_test
ctest --test-dir build-windows/cpp -R core_hello_actions_test --output-on-failure
```

Expected: configure/build fails because the test target and action do not exist.

- [ ] **Step 3: Implement core identity**

`src/core/lifecycle/core_identity.hpp` should define:

```cpp
namespace exv::core::lifecycle {
struct CoreIdentity {
  std::string core_instance_id;
  int pid = 0;
  std::string core_path;
  std::string started_at;
};

CoreIdentity make_core_identity();
nlohmann::json core_hello_payload(const CoreIdentity& identity);
bool accepts_contract_version(std::string_view requested);
}
```

`core_instance_id` must be random enough for diagnostics, not a security token. Use existing random bytes if available; fall back to timestamp and pid only in tests where the random provider fails.

- [ ] **Step 4: Register `core.hello`**

`CoreActions::register_handlers()` registers `core.hello`. It parses optional payload JSON, accepts an empty payload or a matching `contract_version`, and rejects a mismatching contract with `unsupported_contract_version`.

Wire `CoreActions` into `create_dispatcher()` and retain the action object.

- [ ] **Step 5: Update lifecycle wire test**

Add a native-envelope `core.hello` request to `tests/core_process_lifecycle_test.cpp` and assert the response includes the version and instance fields.

- [ ] **Step 6: Verify and commit**

Run:

```powershell
cmake --build --preset windows-release --target core_hello_actions_test core_process_lifecycle_test
ctest --test-dir build-windows/cpp -R "core_hello_actions_test|core_process_lifecycle_test" --output-on-failure
git add src/core/lifecycle src/core/rpc/core_actions.hpp src/core/rpc/core_actions.cpp src/core/rpc/core_api_setup.cpp src/core/core_process.cpp CMakeLists.txt tests/core_api/core_hello_actions_test.cpp tests/core_process_lifecycle_test.cpp
git commit -m "core: add hello contract action"
```

## Task 3: Versioned IPC Path, Core Lock, And Diagnostic Registry

**Files:**
- Create: `src/core/lifecycle/core_paths.hpp`
- Create: `src/core/lifecycle/core_paths.cpp`
- Create: `src/core/lifecycle/core_lock.hpp`
- Create: `src/core/lifecycle/core_lock.cpp`
- Create: `src/core/lifecycle/core_registry.hpp`
- Create: `src/core/lifecycle/core_registry.cpp`
- Modify: `src/core/pipe_ipc.hpp`
- Modify: `src/core/pipe_ipc.cpp`
- Modify: `src/core/core_process.cpp`
- Modify: `src/helper/runtime/cleanup_registry.*`
- Modify: `CMakeLists.txt`
- Create: `tests/core_lifecycle_registry_test.cpp`
- Modify: `tests/core_process_lifecycle_test.cpp`
- Modify: `tests/helper_cleanup_registry_test.cpp`

- [ ] **Step 1: Write failing lifecycle tests**

Create `tests/core_lifecycle_registry_test.cpp` with cases:

- `core_ipc_path` contains `ipc-v1`.
- `core_lock_path` contains `ipc-v1`.
- A second lock acquisition fails while the first lock is alive.
- Registry write uses temp-file plus atomic rename and can be recreated from an in-memory `CoreRegistrySnapshot`.
- Corrupt registry parse returns `unknown_state`.
- Compare-and-delete refuses to delete when any of `core_instance_id`, `pid`, `helper_core_lease_id`, or `ipc_protocol_version` differs.

- [ ] **Step 2: Verify RED**

Run:

```powershell
cmake --build --preset windows-release --target core_lifecycle_registry_test
ctest --test-dir build-windows/cpp -R core_lifecycle_registry_test --output-on-failure
```

Expected: target does not build until lifecycle files and CMake entries exist.

- [ ] **Step 3: Implement path and lock primitives**

`core_paths` should expose:

```cpp
std::string ipc_protocol_name();      // "ipc-v1"
std::string core_ipc_path();
std::string core_lock_path();
std::string core_registry_path();
```

Replace `core_pipe_path()` internals with `core_ipc_path()` while preserving the public function for existing callers.

`CoreInstanceLock` owns the OS lock for the lifetime of core. On Windows use a named mutex or lock file with exclusive sharing; on POSIX use an exclusive lock file. Tests may use a temporary state directory.

- [ ] **Step 4: Implement registry snapshot**

Registry JSON must include:

```json
{
  "core_instance_id": "string",
  "pid": 0,
  "core_path": "string",
  "ipc_path": "string",
  "ipc_protocol_version": "ipc-v1",
  "app_version": "string",
  "contract_version": "string",
  "started_at": "string",
  "last_heartbeat_at": "string",
  "last_known_tunnel_phase": "idle",
  "last_known_connected": false,
  "last_known_network_ready": false,
  "helper_core_lease_id": ""
}
```

Do not treat missing or corrupt registry files as proof that no core exists.

- [ ] **Step 5: Wire core startup/shutdown**

In `core_process_main()`:

- Bootstrap runtime paths.
- Acquire versioned lock before opening IPC listener.
- Initialize identity.
- Write registry before accepting requests.
- Refresh registry heartbeat periodically and on status callback.
- Stop heartbeat and release lock on shutdown.

- [ ] **Step 6: Extend helper cleanup**

Update helper registry cleanup so it compare-and-deletes only matching versioned registry files after existing cleanup succeeds. Preserve registry on partial cleanup.

- [ ] **Step 7: Verify and commit**

Run:

```powershell
cmake --build --preset windows-release --target core_lifecycle_registry_test core_process_lifecycle_test helper_cleanup_registry_test
ctest --test-dir build-windows/cpp -R "core_lifecycle_registry_test|core_process_lifecycle_test|helper_cleanup_registry_test" --output-on-failure
git add src/core/lifecycle src/core/pipe_ipc.hpp src/core/pipe_ipc.cpp src/core/core_process.cpp src/helper/runtime/cleanup_registry.hpp src/helper/runtime/cleanup_registry.cpp CMakeLists.txt tests/core_lifecycle_registry_test.cpp tests/core_process_lifecycle_test.cpp tests/helper_cleanup_registry_test.cpp
git commit -m "core: add versioned ipc lock and registry"
```

## Task 4: Shared Core Resolver

**Files:**
- Create: `src/platform/common/core_resolver.hpp`
- Create: `src/platform/common/core_resolver.cpp`
- Modify: `src/app/ui_shell/core_process_manager.*`
- Modify: `src/app/ui_shell/core_rpc_client.*`
- Modify: `src/cli/pipe_client.*`
- Modify: `CMakeLists.txt`
- Create: `tests/core_resolver_test.cpp`
- Modify: `tests/ui_shell_core_rpc_client_test.cpp`

- [ ] **Step 1: Write failing resolver tests**

`tests/core_resolver_test.cpp` should use injected dependencies for filesystem, process probe, pipe hello, lock owner, and launcher. Test:

- Existing live IPC with compatible `core.hello` returns `reuse`.
- Unavailable IPC plus no live lock returns `launch`.
- Live lock plus unavailable IPC returns `core_comm_broken`.
- IPC hello timeout after one retry returns `core_unresponsive`.
- Contract mismatch returns `core_protocol_mismatch`.
- `EXV_CORE_PATH` is interpreted as a directory containing `exv.exe` or `exv`.
- A candidate equal to the current frontend executable is rejected.
- Version probe accepts only uncolored machine-readable core version output.

- [ ] **Step 2: Verify RED**

Run:

```powershell
cmake --build --preset windows-release --target core_resolver_test
ctest --test-dir build-windows/cpp -R core_resolver_test --output-on-failure
```

Expected: target does not build until resolver exists.

- [ ] **Step 3: Implement resolver contracts**

Define:

```cpp
enum class CoreResolveStatus {
  ReuseExisting,
  LaunchRequired,
  CoreCommBroken,
  CoreUnresponsive,
  CoreProtocolMismatch,
  CoreNotFound,
  CoreLaunchFailed,
  CoreVersionProbeFailed
};

struct CoreResolveResult {
  CoreResolveStatus status;
  std::string ipc_path;
  std::string core_path;
  nlohmann::json hello;
  nlohmann::json registry_snapshot;
  std::string message;
};
```

Resolver order must match the design: IPC hello, lock inspection, executable discovery, launch.

- [ ] **Step 4: Wire UI shell to resolver**

`core_process_manager` should reuse the shared resolver for startup classification. The UI must not kill a live residual core automatically.

- [ ] **Step 5: Verify and commit**

Run:

```powershell
cmake --build --preset windows-release --target core_resolver_test ui_shell_core_rpc_client_test
ctest --test-dir build-windows/cpp -R "core_resolver_test|ui_shell_core_rpc_client_test" --output-on-failure
git add src/platform/common/core_resolver.hpp src/platform/common/core_resolver.cpp src/app/ui_shell/core_process_manager.hpp src/app/ui_shell/core_process_manager.cpp src/app/ui_shell/core_rpc_client.hpp src/app/ui_shell/core_rpc_client.cpp src/cli/pipe_client.hpp src/cli/pipe_client.cpp CMakeLists.txt tests/core_resolver_test.cpp tests/ui_shell_core_rpc_client_test.cpp
git commit -m "platform: add shared core resolver"
```

## Task 5: Maintenance Inspect And Kill-Stale-Core Actions

**Files:**
- Create: `src/core/rpc/maintenance_actions.hpp`
- Create: `src/core/rpc/maintenance_actions.cpp`
- Modify: `src/core/rpc/core_api_setup.cpp`
- Modify: `src/platform/common/core_resolver.*`
- Modify: `src/app/ui_shell/core_process_manager.*`
- Modify: `CMakeLists.txt`
- Create: `tests/core_api/maintenance_actions_test.cpp`
- Modify: `tests/core_resolver_test.cpp`

- [ ] **Step 1: Write failing tests**

Test:

- `maintenance.inspectCore` returns registry-derived state and risk level without mutating anything.
- `maintenance.killStaleCore` with missing or false `confirm` returns `confirmation_required`.
- Confirmed kill prefers IPC shutdown when communication is possible.
- Confirmed kill by PID verifies process dead, lock released, and IPC unavailable before returning success.
- Unknown registry state is reported as unknown risk.

- [ ] **Step 2: Verify RED**

Run:

```powershell
cmake --build --preset windows-release --target maintenance_actions_test
ctest --test-dir build-windows/cpp -R maintenance_actions_test --output-on-failure
```

- [ ] **Step 3: Implement maintenance action adapters**

Keep termination behavior behind dependency-injected process control for tests. Do not terminate the current test process in unit tests.

- [ ] **Step 4: Verify and commit**

Run:

```powershell
cmake --build --preset windows-release --target maintenance_actions_test core_resolver_test
ctest --test-dir build-windows/cpp -R "maintenance_actions_test|core_resolver_test" --output-on-failure
git add src/core/rpc/maintenance_actions.hpp src/core/rpc/maintenance_actions.cpp src/core/rpc/core_api_setup.cpp src/platform/common/core_resolver.hpp src/platform/common/core_resolver.cpp src/app/ui_shell/core_process_manager.hpp src/app/ui_shell/core_process_manager.cpp CMakeLists.txt tests/core_api/maintenance_actions_test.cpp tests/core_resolver_test.cpp
git commit -m "core: add stale core maintenance actions"
```

## Task 6: Core-Owned Config, Key, Routes, Retry, And Reset Semantics

**Files:**
- Modify: `src/core/use_cases/config_use_cases.hpp`
- Modify: `src/core/use_cases/config_use_cases.cpp`
- Modify: `src/core/rpc/config_actions.hpp`
- Modify: `src/core/rpc/config_actions.cpp`
- Modify: `src/core/rpc/route_actions.*`
- Modify: `src/core/rpc/vpn_actions.*`
- Modify: `src/core/tunnel_controller/*`
- Modify: `src/core/config/config.hpp`
- Modify: `src/core/config/config_api.*`
- Modify: `CMakeLists.txt`
- Modify: `tests/core_api/config_actions_test.cpp`
- Modify: `tests/core_api/route_actions_test.cpp`
- Modify: `tests/reconnect_policy_test.cpp`
- Create: `tests/core_api/config_reset_session_snapshot_test.cpp`

- [ ] **Step 1: Write failing config tests**

Extend `tests/core_api/config_actions_test.cpp`:

- `config.get` returns `auth`, `settings`, `routes`, and no key path/fingerprint/material.
- `config.saveAuth` updates only auth fields and preserves settings.
- `config.saveSettings` accepts `retry_limit` where `-1`, `0`, and positive integers are valid.
- `config.saveSettings` rejects `retry_limit < -1`.
- `config.reset` without `confirm:true` returns `confirmation_required`.
- `config.reset` with `confirm:true` resets persisted config and does not call VPN disconnect hooks.
- `key.status` returns only `available`, `present`, and `status`.
- `key.reset` without `confirm:true` returns `confirmation_required`.
- `key.reset` clears persisted encrypted password and does not expose key path.
- `routes.reset` works through `routes.reset`.

- [ ] **Step 2: Write failing active-session snapshot test**

Create `tests/core_api/config_reset_session_snapshot_test.cpp` that starts a fake active session, changes persisted config, and verifies `status.get` reports active session server, username, route count, MTU, and retry policy from the captured session snapshot.

- [ ] **Step 3: Verify RED**

Run:

```powershell
cmake --build --preset windows-release --target config_actions_test route_actions_test config_reset_session_snapshot_test
ctest --test-dir build-windows/cpp -R "config_actions_test|route_actions_test|config_reset_session_snapshot_test" --output-on-failure
```

- [ ] **Step 4: Implement action names and confirmations**

Register new contract names directly:

- `config.get`
- `config.saveAuth`
- `config.saveSettings`
- `config.reset`
- `key.status`
- `key.reset`
- `routes.list`
- `routes.add`
- `routes.remove`
- `routes.reset`

Keep legacy aliases only as compatibility shims until Task 8 removes user-facing legacy CLI paths.

- [ ] **Step 5: Implement retry capture**

Add `retry_limit` to persistent config if missing. `vpn.connect` must read config at connection start and capture retry policy in the active session snapshot. Later config saves must not mutate the active snapshot.

- [ ] **Step 6: Verify and commit**

Run:

```powershell
cmake --build --preset windows-release --target config_actions_test route_actions_test vpn_actions_test config_reset_session_snapshot_test
ctest --test-dir build-windows/cpp -R "config_actions_test|route_actions_test|vpn_actions_test|config_reset_session_snapshot_test" --output-on-failure
git add src/core/use_cases/config_use_cases.hpp src/core/use_cases/config_use_cases.cpp src/core/rpc/config_actions.hpp src/core/rpc/config_actions.cpp src/core/rpc/route_actions.hpp src/core/rpc/route_actions.cpp src/core/rpc/vpn_actions.hpp src/core/rpc/vpn_actions.cpp src/core/tunnel_controller src/core/config CMakeLists.txt tests/core_api/config_actions_test.cpp tests/core_api/route_actions_test.cpp tests/core_api/vpn_actions_test.cpp tests/core_api/config_reset_session_snapshot_test.cpp tests/reconnect_policy_test.cpp
git commit -m "core: centralize config key route and retry actions"
```

## Task 7: Config Import And Export Contract Format

**Files:**
- Create: `src/core/config/config_export.hpp`
- Create: `src/core/config/config_export.cpp`
- Create: `src/core/config/secure_buffer.hpp`
- Modify: `src/core/use_cases/config_use_cases.*`
- Modify: `src/core/rpc/config_actions.*`
- Modify: `src/platform/common/crypto_backend.hpp`
- Modify platform crypto backends only if required for MAC/AEAD/KDF support.
- Modify: `CMakeLists.txt`
- Create: `tests/config_import_export_test.cpp`
- Modify: `tests/security/no_secret_in_logs_test.cpp`

- [ ] **Step 1: Write failing import/export tests**

`tests/config_import_export_test.cpp` must cover:

- Old flat JSON import rejects with `config_import_format_unsupported`.
- Non-sensitive export excludes VPN password and omits `protection.password`.
- Password-protected export includes visible config, KDF metadata, MAC metadata, AEAD metadata, and no local key material.
- Correct export password imports and re-encrypts VPN password with the local key.
- Wrong password rejects with `config_import_tampered_or_wrong_password`.
- Modified visible server or username rejects because body authentication fails.
- Local key is not written until protected import validates and key creation is confirmed.
- Temporary plaintext buffer zeroization path is exercised by a test-only observer compiled into the test target, not into production.

- [ ] **Step 2: Verify RED**

Run:

```powershell
cmake --build --preset windows-release --target config_import_export_test
ctest --test-dir build-windows/cpp -R config_import_export_test --output-on-failure
```

- [ ] **Step 3: Resolve crypto dependency explicitly**

The design requires Argon2id, keyed MAC, and XChaCha20-Poly1305 AEAD. Before implementation, check whether libsodium headers/library are available in the Windows toolchain. If unavailable, add a narrow CMake feature probe and return a structured `config_import_auth_failed` / `config_import_tampered_or_wrong_password` only for runtime data failures, not missing build dependencies. Do not silently replace the required algorithms with AES-CBC.

- [ ] **Step 4: Implement canonical body and non-sensitive export**

Canonicalize visible config with stable key order. Non-sensitive export must be useful without a password and must not include persisted encrypted password.

- [ ] **Step 5: Implement protected export/import**

Use secure buffers for derived keys and temporary plaintext. Zeroize temporary plaintext after re-encryption or failed import.

- [ ] **Step 6: Verify and commit**

Run:

```powershell
cmake --build --preset windows-release --target config_import_export_test config_actions_test no_secret_in_logs_test
ctest --test-dir build-windows/cpp -R "config_import_export_test|config_actions_test|no_secret_in_logs_test" --output-on-failure
git add src/core/config/config_export.hpp src/core/config/config_export.cpp src/core/config/secure_buffer.hpp src/core/use_cases/config_use_cases.hpp src/core/use_cases/config_use_cases.cpp src/core/rpc/config_actions.hpp src/core/rpc/config_actions.cpp src/platform/common/crypto_backend.hpp src/platform/win32/crypto_backend.cpp src/platform/linux/crypto_backend.cpp src/platform/darwin/crypto_backend.cpp CMakeLists.txt tests/config_import_export_test.cpp tests/security/no_secret_in_logs_test.cpp
git commit -m "core: add contract config import export"
```

## Task 8: Split `exv-cli` And Convert User CLI To IPC

**Files:**
- Create: `src/cli/cli_main.cpp`
- Create: `src/cli/cli_commands.hpp`
- Create: `src/cli/cli_commands.cpp`
- Create: `src/cli/core_request_formatter.hpp`
- Create: `src/cli/core_request_formatter.cpp`
- Modify: `src/app/main.cpp`
- Modify: `src/cli/pipe_client.*`
- Modify: `CMakeLists.txt`
- Create: `tests/cli_commands_test.cpp`
- Modify: `tests/core_architecture_contract_test.cpp`
- Modify: `tests/security/no_secret_in_argv_test.cpp`

- [ ] **Step 1: Write failing CLI tests**

`tests/cli_commands_test.cpp` should not start a real core. Inject a fake core client and assert:

- `exv-cli start` sends `vpn.connect`.
- `exv-cli start -rt 3` sends `config.saveSettings` with `settings.retry_limit = 3`, then `vpn.connect`.
- `exv-cli start -rt` sends `retry_limit = -1`.
- `exv-cli stop` sends `vpn.disconnect`.
- `exv-cli status` sends `status.get`.
- `exv-cli config show` sends `config.get`.
- `exv-cli config set server https://vpn.example.edu` sends `config.saveAuth`.
- `exv-cli config set mtu 1400` sends `config.saveSettings`.
- Unknown config keys fail locally.
- `exv-cli config reset` sends `config.reset` only after interactive confirmation.
- Non-interactive destructive commands fail unless explicit cleanup/confirm flag is present where the command supports one.
- `exv-cli version` performs discovery and sends no business action; it prints core version probe output.

- [ ] **Step 2: Write architecture guard**

Extend `tests/core_architecture_contract_test.cpp` to scan `src/cli` and fail on includes or calls to direct business APIs:

```text
core/config/config_api
core/config/config_manager
core/vpn/vpn.hpp
core/network/virtual_network_status
helper/helper.hpp
app_api::handle_action
```

- [ ] **Step 3: Verify RED**

Run:

```powershell
cmake --build --preset windows-release --target cli_commands_test core_architecture_contract_test
ctest --test-dir build-windows/cpp -R "cli_commands_test|core_architecture_contract_test" --output-on-failure
```

- [ ] **Step 4: Implement thin CLI**

`exv-cli` owns only:

- argument parsing
- core resolving/startup through shared resolver
- core IPC request construction
- terminal formatting
- confirmation prompts

It must not call config, VPN, service, route, log, or key business functions directly.

- [ ] **Step 5: Keep `exv` core-only**

`exv` keeps:

- `--mode=core`
- `--version` machine-readable core version probe
- `__tunnel-script`
- temporary `desktop-rpc` compatibility shims if still required by UI migration

Normal user CLI help should point to `exv-cli`.

- [ ] **Step 6: Verify and commit**

Run:

```powershell
cmake --build --preset windows-release --target exv exv-cli cli_commands_test core_architecture_contract_test no_secret_in_argv_test
ctest --test-dir build-windows/cpp -R "cli_commands_test|core_architecture_contract_test|no_secret_in_argv_test" --output-on-failure
git add src/cli src/app/main.cpp CMakeLists.txt tests/cli_commands_test.cpp tests/core_architecture_contract_test.cpp tests/security/no_secret_in_argv_test.cpp
git commit -m "cli: split exv-cli as ipc frontend"
```

## Task 9: UI Parity Controls And Maintenance Prompts

**Files:**
- Modify: `webui/src/pages/SettingsPage.vue`
- Modify: `webui/src/pages/RoutesPage.vue`
- Modify: `webui/src/stores/config.ts`
- Modify: `webui/src/api/desktop.ts`
- Modify: `webui/src/api/host.ts`
- Modify: `webui/desktop/shared/desktop-contract.ts`
- Modify: `webui/host/shared/host-contract.ts`
- Modify: `webui/host/__tests__/host-boundary.test.ts`
- Modify: `webui/host/__tests__/desktop-contract-generated.test.ts`
- Add component tests if an existing Vue test harness is present.

- [ ] **Step 1: Write failing host/contract tests**

Update host tests to require actions:

- `config.import`
- `config.export`
- `config.reset`
- `key.reset`
- `maintenance.inspectCore`
- `maintenance.killStaleCore`

Assert renderer-facing types do not expose key path, fingerprint, or key material.

- [ ] **Step 2: Implement UI action plumbing**

Expose:

- config import
- non-sensitive config export
- password-protected config export
- config reset
- key reset
- retry limit setting
- broken core maintenance prompt

Use existing `ConfirmDialog`, `PasswordPromptDialog`, and toast patterns. Do not add a landing page or explanatory feature panels.

- [ ] **Step 3: Add warning copy**

After password-protected export, show this exact warning text:

```text
The exported configuration contains a recoverable VPN password. Treat it as a
sensitive file. Do not share it through untrusted channels or copy it to public
machines. The password is not stored in plaintext, but a weak export password
can still be attacked offline. Destroy the file after use when possible.
```

- [ ] **Step 4: Verify and commit**

Run:

```powershell
cd webui
pnpm run test:host
pnpm run build
cd ..
git add webui/src webui/desktop/shared webui/host/shared webui/host/__tests__
git commit -m "ui: add config maintenance parity controls"
```

## Task 10: Full Verification And Final Review

**Files:**
- No planned production edits.
- Modify docs only if verification reveals stale user-facing behavior.

- [ ] **Step 1: Run contract generation check**

```powershell
python scripts/generate_contracts.py --check
```

- [ ] **Step 2: Run frontend checks**

```powershell
cd webui
pnpm run test:host
pnpm run build
cd ..
```

- [ ] **Step 3: Run release-blocking suite**

```powershell
./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking
```

- [ ] **Step 4: Run architecture guardrails**

```powershell
./scripts/architecture-guardrails.ps1
```

- [ ] **Step 5: Dispatch final reviewers**

Dispatch one final spec reviewer and one final code-quality reviewer over the full branch range from `a40ac06` to `HEAD`.

- [ ] **Step 6: Final commit if needed**

If verification causes doc or cleanup changes:

```powershell
git add <changed-files>
git commit -m "chore: finalize cli core ui contract refactor"
```
