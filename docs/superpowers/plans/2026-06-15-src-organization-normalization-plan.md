# Src Organization Normalization Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `src/` match normal cross-platform C++ project organization: no root-level compatibility shims, no source include-units, no generated payloads in source roots, no platform-to-core dependency inversion, and clear targets/modules per domain.

**Architecture:** Use `src/common` for cross-cutting low-level infrastructure, `src/app` for process entrypoints, `src/core` for business orchestration, `src/platform` for OS adapters, `src/helper` for helper protocol/runtime, `src/vpn_engine` for protocol/engine code, `src/utils` for pure value helpers, and build-generated include directories for generated artifacts. Each phase adds tests or contract gates before moving files, then updates consumers and CMake in small commits.

**Tech Stack:** C++20, CMake 3.28, Ninja/MSVC or MinGW, existing CTest suite, Python standard-library generation scripts only where already used.

---

## Current Findings

The following items are still structurally misplaced or confusing after the helper/config/tunnel/utils work.

### Root-Level `src` Remnants

These files sit directly under `src/` and hide real ownership:

- Compatibility shims:
  - `src/app_api.hpp`
  - `src/app_api_native_orchestration.hpp`
  - `src/connection_attempt.hpp`
  - `src/crypto.hpp`
  - `src/error_types.hpp`
  - `src/log_event_bus.hpp`
  - `src/log_renderer.hpp`
  - `src/logger.hpp`
  - `src/vpn.hpp`
  - `src/vpn_legacy_adapter.hpp`
- Real root files that should move:
  - `src/main.cpp`
  - `src/tunnel.hpp`
  - `src/tunnel.cpp`
  - `src/virtual_network.hpp`
  - `src/virtual_network.cpp`
  - `src/openconnect_log.hpp`
  - `src/openconnect_log.cpp`
- Generated payload in source root:
  - `src/webui_assets.hpp`

### Shadow Public API Directories

`src/core_api/` contains only compatibility shims to `src/core/rpc/`:

- `src/core_api/app_rpc_dispatcher.hpp`
- `src/core_api/config_actions.hpp`
- `src/core_api/core_api_setup.hpp`
- `src/core_api/desktop_rpc_adapter.hpp`
- `src/core_api/route_actions.hpp`
- `src/core_api/service_actions.hpp`
- `src/core_api/vpn_actions.hpp`

This makes it unclear whether `core_api` or `core/rpc` owns the public contract.

### Core Root Compatibility Shims

`src/core/` still has tunnel-controller forwarding headers:

- `src/core/core_error_mapper.hpp`
- `src/core/core_session_runner.hpp`
- `src/core/engine_event_bridge.hpp`
- `src/core/native_startup_failure.hpp`
- `src/core/reconnect_policy.hpp`
- `src/core/timer_scheduler.hpp`
- `src/core/tunnel_controller.hpp`
- `src/core/tunnel_events.hpp`
- `src/core/tunnel_intent.hpp`
- `src/core/tunnel_state.hpp`

These should either become the canonical public include path or disappear. They should not remain accidental aliases forever.

### Source Include-Units

There are still 48 `*.inc.cpp` files included from `.cpp` files. The largest clusters are:

- `src/core/app_api/*inc.cpp`
- `src/core/config/*inc.cpp`
- `src/core/connection/*inc.cpp`
- `src/vpn_engine/protocol/production_transport_*inc.cpp`
- `src/platform/win32/native_*inc.cpp`
- `src/platform/win32/tunnel_script_*inc.cpp`
- `src/platform/darwin/native_*inc.cpp`
- `src/platform/darwin/helper_lifecycle_*inc.cpp`

Include-units make ownership, build dependencies, symbol visibility, testing, and module migration harder. They should be replaced with normal `.cpp` files plus private headers where needed.

### Platform Boundary Inversions

`src/platform` still includes high-level root or core-facing facades such as:

- `logger.hpp`
- `vpn.hpp`
- `tunnel.hpp`
- `openconnect_log.hpp`
- `virtual_network.hpp`

Examples:

- `src/platform/linux/app_api_runtime_policy.cpp` includes `vpn.hpp`
- `src/platform/darwin/app_api_runtime_policy.cpp` includes `vpn.hpp`
- `src/platform/linux/helper_lifecycle.cpp` includes `tunnel.hpp`
- `src/platform/darwin/helper_lifecycle.cpp` includes `tunnel.hpp`
- `src/platform/win32/tunnel_script.cpp` includes `openconnect_log.hpp`
- `src/platform/common/proxy_tun_detector.hpp` includes `virtual_network.hpp`

In the final organization, `platform` may implement platform interfaces and may implement `vpn_engine` low-level interfaces, but it must not depend on `core`, `helper` runtime, application RPC, CLI, or business orchestration facades.

### CMake Structure

`CMakeLists.txt` still has large global source lists and many standalone tests manually linking implementation files. That pattern keeps old file locations alive and makes module boundaries weak.

### Namespace Mixed State

Both `ecnuvpn` and `exv` namespaces are used across new and old code. This is not immediately broken, but it is a long-term source of confusion. Final normalization should isolate legacy aliases and make new canonical module APIs use `exv::*`.

---

## Target Layout

The end state should look like this:

```text
src/
  app/
    main.cpp
    cli_main.cpp
  common/
    diagnostics/
      logger.hpp
      logger.cpp
      log_event_bus.hpp
      log_event_bus.cpp
      log_renderer.hpp
      log_renderer.cpp
    errors/
      error_types.hpp
    generated/
      generated_anchor.hpp
  core/
    app_api/
    config/
    connection/
    native_orchestration/
    rpc/
    tunnel_controller/
    vpn/
  helper/
    common/
    runtime/
    modules/
  platform/
    common/
    darwin/
    linux/
    win32/
  runtime/
  utils/
    strings.hpp
    strings.cpp
    modules/
  vpn_engine/
    openconnect/
      openconnect_log.hpp
      openconnect_log.cpp
    protocol/
```

Generated WebUI assets should not be committed under `src/` root. The generated header should be written under the build tree, for example:

```text
build/generated/webui_assets.hpp
```

and included through a generated include directory.

---

## Global Acceptance Gates

The final state is accepted only when all gates pass:

- `src/` root contains directories only, except for explicitly documented generated anchors if still needed.
- `src/webui_assets.hpp` does not exist.
- `src/core_api/` does not exist.
- `src/*.hpp` compatibility shims do not exist.
- `src/core/*` tunnel-controller compatibility shims do not exist.
- `rg -n '#include ".*\.inc\.cpp"' src` returns no results.
- `Get-ChildItem src -Recurse -Filter *.inc.cpp` returns no files.
- `src/platform` does not include `app_api.hpp`, `vpn.hpp`, `tunnel.hpp`, `logger.hpp`, `openconnect_log.hpp`, `virtual_network.hpp`, or `core/`.
- CMake exposes domain targets instead of one large `EXV_COMMON_SOURCES` list.
- Tests link domain targets, not hand-picked production implementation files, except where a test is explicitly a link-isolation contract.
- New public module-facing APIs use `exv::*`; any remaining `ecnuvpn::*` compatibility is isolated and gated.
- `cmake --build build --config Debug` passes.
- `ctest --test-dir build -C Debug --output-on-failure` passes.
- `git diff --check` passes.

---

## Phase 0: Inventory Manifest And Boundary Gates

**Purpose:** Freeze the current structural debt as testable facts before moving more files.

**Files:**

- Modify: `contracts/system.contract.json`
- Modify: `scripts/generate_contracts.py` only if manifest schema needs a new structural field
- Modify: `tests/contract_manifest_test.cpp`
- Modify: `contracts/generated/system_contract_snapshot.json`
- Modify: `src/contracts/generated/system_contract.hpp`
- Modify: `webui/desktop/shared/generated/system-contract.ts`

### Task 0.1: Add Src Organization Contract Entries

- [ ] Add `modules.src_organization` to `contracts/system.contract.json`.
- [ ] Record allowed top-level directories:
  - `app`
  - `cli`
  - `common`
  - `contracts`
  - `core`
  - `feedback`
  - `helper`
  - `platform`
  - `runtime`
  - `utils`
  - `vpn_engine`
- [ ] Record forbidden patterns:
  - `src/*.hpp`
  - `src/*.cpp` except `src/app/main.cpp` after migration
  - `src/core_api`
  - `*.inc.cpp`
  - `#include ".*.inc.cpp"`
  - platform includes of core/app/helper runtime facades
  - committed `src/webui_assets.hpp`

Run:

```powershell
python scripts/generate_contracts.py
```

Expected: generated contract files update cleanly.

### Task 0.2: Add RED Contract Tests

- [ ] Add tests in `tests/contract_manifest_test.cpp` for:
  - root-level compatibility shims still existing
  - `src/core_api` still existing
  - `.inc.cpp` files still existing
  - `src/platform` still including high-level facades
  - `src/webui_assets.hpp` still existing

Run:

```powershell
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^contract_manifest_test$"
```

Expected before implementation: FAIL, with messages naming the remaining structural violations.

### Task 0.3: Commit The RED Gates

Run:

```powershell
git add contracts tests scripts src/contracts/generated webui/desktop/shared/generated
git commit -m "test: add src organization boundary gates"
```

---

## Phase 1: Extract Common Infrastructure

**Purpose:** Remove the reason for platform and other low-level code to include root-level logging/error facades.

**Files:**

- Move: `src/core/logging/logger.hpp` -> `src/common/diagnostics/logger.hpp`
- Move: `src/core/logging/logger.cpp` -> `src/common/diagnostics/logger.cpp`
- Move: `src/core/logging/log_event_bus.hpp` -> `src/common/diagnostics/log_event_bus.hpp`
- Move: `src/core/logging/log_event_bus.cpp` -> `src/common/diagnostics/log_event_bus.cpp`
- Move: `src/core/logging/log_renderer.hpp` -> `src/common/diagnostics/log_renderer.hpp`
- Move: `src/core/logging/log_renderer.cpp` -> `src/common/diagnostics/log_renderer.cpp`
- Move: `src/core/error_types.hpp` -> `src/common/errors/error_types.hpp`
- Move: `src/openconnect_log.hpp` -> `src/vpn_engine/openconnect/openconnect_log.hpp`
- Move: `src/openconnect_log.cpp` -> `src/vpn_engine/openconnect/openconnect_log.cpp`
- Modify: all consumers
- Modify: `CMakeLists.txt`

### Task 1.1: Move Diagnostics

- [ ] Move logging files into `src/common/diagnostics`.
- [ ] Update includes from root shims to:

```cpp
#include "common/diagnostics/logger.hpp"
#include "common/diagnostics/log_event_bus.hpp"
#include "common/diagnostics/log_renderer.hpp"
```

- [ ] Keep old root shims for this phase only.

Run:

```powershell
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(helper_contract_test|app_api_rpc_dispatcher_test|tunnel_controller_integration_test|contract_manifest_test)$"
```

Expected: all selected tests pass except RED gates that intentionally track remaining shims.

### Task 1.2: Move Shared Error Types

- [ ] Move `src/core/error_types.hpp` to `src/common/errors/error_types.hpp`.
- [ ] Update includes to `common/errors/error_types.hpp`.
- [ ] Keep old root/core shim for this phase only.

Run:

```powershell
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(error_contract_test|no_secret_in_logs_test|contract_manifest_test)$"
```

Expected: focused tests pass except intentional RED structural gates.

### Task 1.3: Move OpenConnect Log Parser

- [ ] Move `openconnect_log.*` to `src/vpn_engine/openconnect/`.
- [ ] Rename namespace only if all consumers can be updated in this phase without compatibility wrappers:

```cpp
namespace exv::vpn_engine::openconnect {
```

- [ ] If namespace migration is too large for this phase, keep `ecnuvpn::openconnect_log` and document it as a legacy namespace scheduled for Phase 7.
- [ ] Update platform tunnel script consumers to include:

```cpp
#include "vpn_engine/openconnect/openconnect_log.hpp"
```

Run:

```powershell
cmake --build build --target openconnect_log_test tunnel_script_contract_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(openconnect_log_test|tunnel_script_contract_test)$"
```

Expected: all tests pass.

### Task 1.4: Commit Common Infrastructure

Run:

```powershell
git add -A
git commit -m "refactor: move common diagnostics and openconnect parser"
```

---

## Phase 2: Remove Root Compatibility Shims And Generated Root Payload

**Purpose:** Make canonical include paths visible and remove root clutter.

**Files:**

- Move: `src/main.cpp` -> `src/app/main.cpp`
- Remove root shims:
  - `src/app_api.hpp`
  - `src/app_api_native_orchestration.hpp`
  - `src/connection_attempt.hpp`
  - `src/crypto.hpp`
  - `src/error_types.hpp`
  - `src/log_event_bus.hpp`
  - `src/log_renderer.hpp`
  - `src/logger.hpp`
  - `src/vpn.hpp`
  - `src/vpn_legacy_adapter.hpp`
- Move generated WebUI output out of `src/`
- Modify: `scripts/embed_assets.py`
- Modify: `CMakeLists.txt`
- Modify: consumers under `src` and `tests`

### Task 2.1: Update Includes To Canonical Paths

- [ ] Replace root facade includes:

```cpp
#include "app_api.hpp"
#include "crypto.hpp"
#include "logger.hpp"
#include "vpn.hpp"
#include "tunnel.hpp"
#include "virtual_network.hpp"
```

with canonical includes such as:

```cpp
#include "core/app_api/app_api.hpp"
#include "core/crypto/crypto.hpp"
#include "common/diagnostics/logger.hpp"
#include "core/vpn/vpn.hpp"
#include "core/vpn/openconnect_tunnel_script.hpp"
#include "core/network/virtual_network_status.hpp"
```

- [ ] Use `rg -n '#include "(app_api|crypto|logger|vpn|tunnel|virtual_network|openconnect_log)\.hpp"' src tests` as the enforcement scan.

Run:

```powershell
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(app_api_rpc_dispatcher_test|crypto_roundtrip_test|tunnel_script_contract_test|vpn_actions_test|contract_manifest_test)$"
```

Expected: compile passes; contract gate still fails only for root files that have not been deleted yet.

### Task 2.2: Move Entrypoint

- [ ] Move `src/main.cpp` to `src/app/main.cpp`.
- [ ] Update `CMakeLists.txt` `SOURCES` to use `src/app/main.cpp`.
- [ ] Keep no forwarding `.cpp` at `src/main.cpp`.

Run:

```powershell
cmake --build build --target exv --config Debug
```

Expected: `exv` links.

### Task 2.3: Move WebUI Generated Header Out Of Source Root

- [ ] Change `scripts/embed_assets.py` output from `src/webui_assets.hpp` to:

```text
build/generated/webui_assets.hpp
```

- [ ] Update CMake to add the generated include directory:

```cmake
target_include_directories(exv PRIVATE ${CMAKE_BINARY_DIR}/generated)
```

- [ ] Update includes to:

```cpp
#include "webui_assets.hpp"
```

from the generated include directory.

- [ ] Delete committed `src/webui_assets.hpp`.

Run:

```powershell
cmake --build build --target embed_assets --config Debug
cmake --build build --target exv --config Debug
```

Expected: generated header appears under `build/generated`, and `src/webui_assets.hpp` does not exist.

### Task 2.4: Delete Root Shims

- [ ] Delete all root compatibility shims listed above.
- [ ] Run:

```powershell
Get-ChildItem src -File
```

Expected after Phase 2: no root-level files except items not yet migrated by Phase 3 (`tunnel.*`, `virtual_network.*`) if those were intentionally deferred.

### Task 2.5: Commit Entrypoint And Root Cleanup

Run:

```powershell
git add -A
git commit -m "refactor: remove root src compatibility shims"
```

---

## Phase 3: Move Remaining Root Domain Files

**Purpose:** Move real root files into owner modules and prevent them from returning.

**Files:**

- Move: `src/tunnel.hpp` -> `src/core/vpn/openconnect_tunnel_script.hpp`
- Move: `src/tunnel.cpp` -> `src/core/vpn/openconnect_tunnel_script.cpp`
- Move: `src/virtual_network.hpp` -> `src/core/network/virtual_network_status.hpp`
- Move: `src/virtual_network.cpp` -> `src/core/network/virtual_network_status.cpp`
- Create: `src/platform/common/virtual_network_model.hpp`
- Modify: `src/platform/common/virtual_network_probe.hpp`
- Modify: `src/platform/common/proxy_tun_detector.hpp`
- Modify: consumers and CMake

### Task 3.1: Split Virtual Network Model From Core Status Presentation

- [ ] Create `src/platform/common/virtual_network_model.hpp` with:

```cpp
#pragma once

#include <string>
#include <vector>

namespace exv::platform {

struct VirtualNetworkAdapterInfo {
  std::string name;
  std::string detail;
  std::string kind;
  std::string role;
  std::string if_index;
  std::string route_reason;
};

struct VirtualNetworkDetection {
  bool detected = false;
  std::vector<VirtualNetworkAdapterInfo> adapters;
  std::string message;
};

} // namespace exv::platform
```

- [ ] Update platform probe APIs to return `exv::platform::VirtualNetworkAdapterInfo` instead of depending on root `virtual_network.hpp`.
- [ ] Keep JSON/status presentation in `src/core/network/virtual_network_status.*`.

Run:

```powershell
cmake --build build --target proxy_tun_detector_test app_api_status_contract_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(proxy_tun_detector_test|app_api_status_contract_test)$"
```

Expected: tests pass.

### Task 3.2: Move Tunnel Script Orchestration Into Core VPN

- [ ] Move `src/tunnel.*` to `src/core/vpn/openconnect_tunnel_script.*`.
- [ ] Update namespace only if compatible with consumers in this phase:

```cpp
namespace ecnuvpn::core::vpn::openconnect_tunnel_script
```

- [ ] If namespace migration would touch too much at once, keep the old namespace for Phase 7 and move only the files now.
- [ ] Update includes to:

```cpp
#include "core/vpn/openconnect_tunnel_script.hpp"
```

Run:

```powershell
cmake --build build --target tunnel_script_contract_test config_actions_test helper_contract_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(tunnel_script_contract_test|config_actions_test|helper_contract_test)$"
```

Expected: tests pass.

### Task 3.3: Enforce Empty Source Root

- [ ] Extend `tests/contract_manifest_test.cpp` so `src/` root may contain directories only.
- [ ] Allow no root-level `.hpp` or `.cpp`.
- [ ] Allow no committed `src/webui_assets.hpp`.

Run:

```powershell
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^contract_manifest_test$"
```

Expected: contract test passes after root cleanup.

### Task 3.4: Commit Root Domain Moves

Run:

```powershell
git add -A
git commit -m "refactor: move remaining root domain files"
```

---

## Phase 4: Collapse `core_api` And Core Root Shims

**Purpose:** Remove duplicate public API surfaces.

**Files:**

- Delete: `src/core_api/*`
- Delete tunnel-controller shims under `src/core/*.hpp`
- Modify: tests and consumers
- Modify: `tests/contract_manifest_test.cpp`

### Task 4.1: Choose Canonical RPC Include Path

Canonical path is:

```cpp
#include "core/rpc/app_rpc_dispatcher.hpp"
#include "core/rpc/config_actions.hpp"
#include "core/rpc/core_api_setup.hpp"
#include "core/rpc/desktop_rpc_adapter.hpp"
#include "core/rpc/route_actions.hpp"
#include "core/rpc/service_actions.hpp"
#include "core/rpc/vpn_actions.hpp"
```

- [ ] Replace all `core_api/...` includes in tests and source.
- [ ] Delete `src/core_api/`.
- [ ] Add contract gate that `src/core_api` does not exist.

Run:

```powershell
cmake --build build --target app_api_rpc_dispatcher_test vpn_actions_test config_actions_test service_actions_test route_actions_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(app_api_rpc_dispatcher_test|vpn_actions_test|config_actions_test|service_actions_test|route_actions_test)$"
```

Expected: tests pass.

### Task 4.2: Remove Core Tunnel Controller Shims

Canonical path is:

```cpp
#include "core/tunnel_controller/tunnel_controller.hpp"
#include "core/tunnel_controller/tunnel_state.hpp"
#include "core/tunnel_controller/tunnel_intent.hpp"
#include "core/tunnel_controller/tunnel_events.hpp"
#include "core/tunnel_controller/reconnect_policy.hpp"
#include "core/tunnel_controller/timer_scheduler.hpp"
#include "core/tunnel_controller/core_session_runner.hpp"
#include "core/tunnel_controller/engine_event_bridge.hpp"
#include "core/tunnel_controller/core_error_mapper.hpp"
#include "core/tunnel_controller/native_startup_failure.hpp"
```

- [ ] Replace all `core/<shim>.hpp` includes in tests and source.
- [ ] Delete shim headers from `src/core/`.
- [ ] Add contract gate that these shim files do not exist.

Run:

```powershell
cmake --build build --target tunnel_controller_integration_test core_process_test core_session_runner_test engine_event_bridge_test startup_failure_test timer_scheduler_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(tunnel_controller_integration_test|core_process_test|core_session_runner_test|engine_event_bridge_test|startup_failure_test|timer_scheduler_test)$"
```

Expected: tests pass.

### Task 4.3: Commit Public Include Cleanup

Run:

```powershell
git add -A
git commit -m "refactor: remove duplicate public include shims"
```

---

## Phase 5: Replace Source Include-Units With Normal Translation Units

**Purpose:** Remove `.inc.cpp` as a production pattern.

This phase should be split by cluster. Each cluster gets a RED contract assertion, move, focused tests, and commit.

### Task 5.1: Core App API Include-Units

**Files:**

- Convert:
  - `src/core/app_api/app_api_json_helpers.inc.cpp`
  - `src/core/app_api/app_api_controller_helpers.inc.cpp`
  - `src/core/app_api/app_api_desktop_handlers.inc.cpp`
- Into:
  - `src/core/app_api/app_api_json_helpers.hpp/.cpp`
  - `src/core/app_api/app_api_controller_helpers.hpp/.cpp`
  - `src/core/app_api/app_api_desktop_handlers.hpp/.cpp`

- [ ] Move internal helper declarations into private headers.
- [ ] Stop including `.inc.cpp` from `app_api.cpp`.
- [ ] Add new `.cpp` files to CMake or module target.

Run:

```powershell
cmake --build build --target app_api_rpc_dispatcher_test app_api_status_contract_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(app_api_rpc_dispatcher_test|app_api_status_contract_test)$"
```

Expected: tests pass.

Commit:

```powershell
git add -A
git commit -m "refactor: split app api include units"
```

### Task 5.2: Core Config Include-Units

**Files:**

- Convert legacy and wizard include-units under `src/core/config/` into focused `.cpp/.hpp` files:
  - `config_wizard_ui`
  - `config_wizard_routes`
  - `config_wizard_flow`
  - `config_persistence_legacy`
  - `config_show_legacy`
  - `config_import_legacy`
  - `config_set_value_legacy`
  - `config_maintenance_legacy`

- [ ] Keep public config APIs in existing config headers.
- [ ] Move only implementation helpers to private headers.
- [ ] Do not change config action names or manifest aliases.

Run:

```powershell
cmake --build build --target config_actions_test crypto_roundtrip_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(config_actions_test|crypto_roundtrip_test)$"
```

Expected: tests pass.

Commit:

```powershell
git add -A
git commit -m "refactor: split config include units"
```

### Task 5.3: Core Connection Include-Units

**Files:**

- Convert:
  - `src/core/connection/connection_attempt_internal.inc.cpp`
  - `src/core/connection/connection_attempt_public.inc.cpp`
- Into:
  - `src/core/connection/connection_attempt_internal.hpp/.cpp`
  - keep public methods in `connection_attempt.cpp` or `connection_attempt_public.cpp`

Run:

```powershell
cmake --build build --target connection_attempt_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^connection_attempt_test$"
```

Expected: tests pass.

Commit:

```powershell
git add -A
git commit -m "refactor: split connection attempt include units"
```

### Task 5.4: VPN Engine Production Transport Include-Units

**Files:**

- Convert `src/vpn_engine/protocol/production_transport_*inc.cpp` into:
  - `production_transport_auth.cpp`
  - `production_transport_cstp.cpp`
  - `production_transport_http.cpp`
  - `production_transport_requests.cpp`
  - `production_transport_redaction.cpp`
  - `production_transport_response_parse.cpp`
  - private header `production_transport_internal.hpp`

Run:

```powershell
cmake --build build --target native_production_transport_test native_protocol_session_test native_tls_stream_contract_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(native_production_transport_test|native_protocol_session_test|native_tls_stream_contract_test)$"
```

Expected: tests pass.

Commit:

```powershell
git add -A
git commit -m "refactor: split production transport include units"
```

### Task 5.5: Platform Include-Units

Split platform include-units by platform and subsystem:

- Windows:
  - `native_packet_device_*inc.cpp`
  - `native_tls_stream_*inc.cpp`
  - `tunnel_script_*inc.cpp`
- macOS:
  - `native_packet_device_*inc.cpp`
  - `native_route_config_*inc.cpp`
  - `native_tls_stream_*inc.cpp`
  - `helper_lifecycle_*inc.cpp`

Each split should create normal private `.cpp` files and private internal headers.

Run on Windows:

```powershell
cmake --build build --target win32_native_packet_device_test win32_native_tls_stream_test win32_platform_network_ops_test tunnel_script_contract_test win32_helper_oneshot_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(win32_native_packet_device_test|win32_native_tls_stream_test|win32_platform_network_ops_test|tunnel_script_contract_test|win32_helper_oneshot_test)$"
```

Run on macOS:

```bash
cmake --build build --target darwin_platform_network_ops_test tunnel_script_contract_test helper_contract_test
ctest --test-dir build --output-on-failure -R '^(darwin_platform_network_ops_test|tunnel_script_contract_test|helper_contract_test)$'
```

Expected: platform focused tests pass on their platform.

Commit per platform:

```powershell
git add -A
git commit -m "refactor: split win32 platform include units"
```

```bash
git add -A
git commit -m "refactor: split darwin platform include units"
```

### Task 5.6: Enforce No Include-Units

- [ ] Extend `tests/contract_manifest_test.cpp`:

```text
No files matching src/**/*.inc.cpp
No source line matching #include ".*.inc.cpp"
```

Run:

```powershell
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^contract_manifest_test$"
```

Expected: contract test passes.

Commit:

```powershell
git add -A
git commit -m "test: enforce no source include units"
```

---

## Phase 6: Fix Platform Dependency Direction

**Purpose:** Make `src/platform` a bottom layer, not a consumer of core/app/helper business orchestration.

### Task 6.1: Remove Platform Dependence On VPN Facade

Current issue:

- `src/platform/linux/app_api_runtime_policy.cpp` includes `vpn.hpp`
- `src/platform/darwin/app_api_runtime_policy.cpp` includes `vpn.hpp`

Target:

- Platform policy returns platform facts only.
- Core/app API decides whether to call VPN fallback behavior.

Files:

- Modify: `src/platform/common/app_api_runtime_policy.hpp`
- Modify: `src/platform/{darwin,linux,win32}/app_api_runtime_policy.cpp`
- Modify: `src/core/app_api/app_api.cpp`
- Modify: `tests/app_api_runtime_policy_test.cpp`

Run:

```powershell
cmake --build build --target app_api_runtime_policy_test app_api_rpc_dispatcher_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(app_api_runtime_policy_test|app_api_rpc_dispatcher_test)$"
```

Expected: tests pass and platform no longer includes `vpn.hpp`.

### Task 6.2: Remove Platform Dependence On Tunnel Facade

Current issue:

- `src/platform/linux/helper_lifecycle.cpp` includes `tunnel.hpp`
- `src/platform/darwin/helper_lifecycle.cpp` includes `tunnel.hpp`

Target:

- Helper lifecycle exposes "cleanup routes" as a callback or delegates to `platform/common/platform_network_ops`.
- Core/helper runtime invokes route cleanup explicitly.
- Platform lifecycle code does not include core VPN tunnel orchestration.

Run:

```powershell
cmake --build build --target helper_contract_test helper_timeout_cleanup_test helper_delegating_network_ops_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(helper_contract_test|helper_timeout_cleanup_test|helper_delegating_network_ops_test)$"
```

Expected: tests pass and platform no longer includes `tunnel.hpp`.

### Task 6.3: Remove Platform Dependence On Root Logger

Target:

- Platform includes `common/diagnostics/logger.hpp` only.
- No platform file includes root `logger.hpp`.

Run:

```powershell
rg -n '#include "logger\.hpp"' src/platform
```

Expected: no output.

### Task 6.4: Add Platform Boundary Contract Gate

- [ ] Extend `tests/contract_manifest_test.cpp` to reject platform includes:

```text
#include "app_api.hpp"
#include "vpn.hpp"
#include "tunnel.hpp"
#include "logger.hpp"
#include "openconnect_log.hpp"
#include "virtual_network.hpp"
#include "core/
#include "helper/helper.hpp"
#include "helper/helper_handler.hpp"
```

Allow:

- `helper/common/*` protocol/message types when needed for IPC boundaries
- `vpn_engine/*` low-level interfaces that platform implements
- `common/*`
- `platform/common/*`

Run:

```powershell
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^contract_manifest_test$"
```

Expected: contract test passes.

### Task 6.5: Commit Platform Direction Fix

Run:

```powershell
git add -A
git commit -m "refactor: enforce platform dependency direction"
```

---

## Phase 7: CMake Target Decomposition

**Purpose:** Make build targets match source boundaries, so future moves do not require hand-linking dozens of files in tests.

### Task 7.1: Introduce Domain Libraries

Create CMake libraries:

- `exv-common-diagnostics`
- `exv-common-errors`
- `exv-core-config`
- `exv-core-rpc`
- `exv-core-vpn`
- `exv-core-app-api`
- `exv-core-tunnel-controller`
- `exv-helper-common`
- `exv-helper-runtime`
- `exv-platform-common`
- `exv-platform-current`
- `exv-vpn-engine`
- `exv-vpn-engine-protocol`

Each target should own its source files and public include directories.

Run:

```powershell
cmake --build build --config Debug
```

Expected: build passes.

### Task 7.2: Stop Standalone Tests From Hand-Linking Production Source Sets

- [ ] Convert tests that manually list production `.cpp` files to link domain targets.
- [ ] Keep source-level tests only when intentionally testing isolated platform code.
- [ ] Document each remaining source-level target with a CMake comment naming the reason.

Run:

```powershell
ctest --test-dir build -C Debug --output-on-failure
```

Expected: all tests pass.

### Task 7.3: Add CMake Boundary Gates

Add contract checks:

- No `EXV_COMMON_SOURCES` monolith.
- No `src/core_api`.
- No `src/*.hpp` shims.
- No standalone test target links deleted root files.

Run:

```powershell
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^contract_manifest_test$"
```

Expected: contract test passes.

Commit:

```powershell
git add -A
git commit -m "refactor: split cmake domain targets"
```

---

## Phase 8: Namespace And Module Normalization

**Purpose:** Make the new source organization visible at API level without breaking all legacy names at once.

### Task 8.1: Namespace Policy

Canonical policy:

- New APIs and modules use `exv::*`.
- Legacy `ecnuvpn::*` stays only where moving it would require broad behavior migration.
- Any compatibility aliases must live in explicitly named compatibility headers under `src/common/compat/`.
- No compatibility alias may live in source root.

Add gates:

- New files under `src/common`, `src/core`, `src/helper`, `src/utils`, and `src/vpn_engine` should use `exv::*`.
- Existing `ecnuvpn::*` files are listed in an allowlist manifest.
- The allowlist must shrink phase-by-phase.

Run:

```powershell
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^contract_manifest_test$"
```

Expected: contract test passes with explicit allowlist.

### Task 8.2: Add Module Facades For Stable Domains

Add narrow named modules after physical layout is clean:

- `exv.common.diagnostics`
- `exv.common.errors`
- `exv.vpn_engine.openconnect`
- `exv.core.rpc.contract`
- `exv.core.vpn.openconnect_tunnel_script`

Each module should export only stable public types/functions and should not export implementation helpers.

Run:

```powershell
cmake --build build --target common_diagnostics_module_smoke_test vpn_engine_openconnect_module_smoke_test core_rpc_module_smoke_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "module_smoke_test$"
```

Expected: module smoke tests pass.

Commit:

```powershell
git add -A
git commit -m "feat: add normalized source module facades"
```

---

## Phase 9: Final Full Acceptance

Run the full verification suite:

```powershell
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
git diff --check
rg -n '#include ".*\.inc\.cpp"' src
Get-ChildItem src -Recurse -Filter *.inc.cpp
Get-ChildItem src -File
Test-Path src\core_api
Test-Path src\webui_assets.hpp
```

Expected:

- Build passes.
- CTest passes.
- `git diff --check` passes.
- `rg -n '#include ".*\.inc\.cpp"' src` returns no output.
- `Get-ChildItem src -Recurse -Filter *.inc.cpp` returns no files.
- `Get-ChildItem src -File` returns no files, or only explicitly documented generated anchor files.
- `Test-Path src\core_api` returns `False`.
- `Test-Path src\webui_assets.hpp` returns `False`.

Final commit:

```powershell
git add -A
git commit -m "refactor: normalize src project organization"
```

---

## Recommended Execution Order

Use subagent-driven development with one worker per phase after Phase 0. Do not run multiple workers on the same file cluster at the same time.

Safe parallelism:

- Phase 1 diagnostics and OpenConnect parser can run independently if file ownership is split.
- Phase 5 platform Win32 and Darwin splits can run independently in separate worktrees.
- Phase 7 CMake decomposition should be single-owner because it touches every target.

Do not start Phase 8 until:

- all root shims are gone,
- all include-units are gone,
- platform dependency direction is clean,
- CMake target decomposition is complete.

---

## Self-Review

- The plan covers every currently observed structural issue: root shims, root domain files, `core_api`, core tunnel shims, include-units, generated WebUI asset, platform dependency inversions, CMake monoliths, and namespace/module drift.
- No step requires changing helper/config/tunnel/user-facing wire contracts.
- Platform remains "works first"; the plan only enforces direction and physical ownership before attempting deeper platform modularization.
- The plan intentionally keeps namespace migration late to avoid mixing physical file moves with API renames.
- Every phase has focused verification and a commit point.
