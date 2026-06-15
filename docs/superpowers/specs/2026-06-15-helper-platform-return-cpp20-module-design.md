# Helper Platform Return and C++20 Module Pilot Design

Date: 2026-06-15

## Boundary Model

`src/helper` is the privileged helper domain. It owns the single helper
protocol, message serialization, handler dispatch, session store, lease
management, cleanup registry, heartbeat policy, and helper runtime behavior.

`src/platform` is the platform integration domain. It owns the helper-facing OS
details needed by core/helper:

- helper endpoint configuration and launch metadata
- service install/uninstall/status integration
- service and oneshot daemon entrypoints
- process, parent-pid, owner, and supervisor lifecycle checks
- platform IPC client used to reach the privileged helper
- adapter from core `PlatformNetworkOps` to helper network messages

This restores the dependency direction expected by the broader architecture:
platform code may adapt helper protocol types at the edge, but helper packages
do not physically own OS integration files.

## File Layout

Common helper platform abstractions live in `src/platform/common`:

- `helper_client.hpp`
- `helper_lifecycle.hpp`
- `helper_platform.hpp`
- `helper_service_manager.hpp`
- `helper_delegating_network_ops.hpp`
- `helper_delegating_network_ops.cpp`

OS implementations live beside the rest of the platform implementation:

- `src/platform/win32/helper_*.cpp`
- `src/platform/darwin/helper_*.cpp`
- `src/platform/linux/helper_*.cpp`

`src/helper/platform` must not be recreated.

## C++20 Module Pilot

The first named module is intentionally narrow:

- module name: `exv.helper.protocol`
- module interface: `src/helper/modules/helper_protocol.cppm`
- exports helper protocol enums and read-only helpers only
- requires the native build generator to support CMake C++ module dependency
  scanning; Linux, macOS, Windows, and Docker builds use Ninja for this path
- requires a module-capable compiler in CI and Docker: GCC 14, Homebrew
  LLVM/Clang, or MSVC from Visual Studio 2022 or newer
- does not export JSON serialization, runtime classes, platform entrypoints, or
  service/oneshot behavior

The smoke test imports the module and compares exported helper op codes and
session requirements with `src/contracts/generated/system_contract.hpp`. This
keeps the pilot tied to the generated contract without migrating the whole
helper runtime into modules.

## Non-Goals

- No helper wire-contract changes.
- No helper heartbeat, cleanup, shutdown, service, or oneshot behavior changes.
- No namespace cleanup beyond what is required for includes and CMake.
- No production mock/stub additions.
- No migration of config, VPN engine, route runtime, logs, or UI modules.

## Acceptance Criteria

- No production or test include uses `helper/platform/...`.
- `CMakeLists.txt` contains no `src/helper/platform` sources.
- `src/platform` does not include helper runtime or handler internals.
- `helper_contract_test` enforces the boundary.
- `helper_module_smoke_test` imports `exv.helper.protocol`.
- Existing helper/core focused tests continue to pass.
