# Helper and Config Package Consolidation Design

**Date:** 2026-06-14
**Status:** Design Approved
**Scope:** `src/helper/**`, `src/core/config/**`, related include paths, and CMake source selection

## 1. Problem Statement

The `helper` and `config` code currently has inconsistent module boundaries:

- `config` is partly under `src/core/config/`, but root-level compatibility headers (`src/config.hpp`, `src/config_api.hpp`, `src/config_manager.hpp`) still expose old paths.
- `config` platform defaults live outside the config package under `src/platform/common/`, `src/platform/darwin/`, and `src/platform/win32/`.
- `helper` is split across root files (`src/helper*.cpp/hpp`), `src/helper_common/`, `src/helper_runtime/`, daemon files in the root, and helper-specific platform code under `src/platform/**`.
- `src/platform/common/` currently mixes cross-package abstractions with platform adapter declarations. This makes ownership unclear and pulls helper/config concerns away from their modules.

The target is a cleaner package layout:

- `helper` is an independent top-level module under `src/helper/`.
- `config` remains a `core` submodule under `src/core/config/`.
- Shared or abstract code stays in the package root or package subdirectories outside `platform/`.
- Only code that hides platform-specific implementation details lives under each package's `platform/` directory.
- Build-time platform selection is controlled by project-defined macros and CMake source selection.

## 2. Decisions Already Approved

1. **Delete compatibility shims.** Do not keep root-level compatibility headers such as `src/config.hpp`, `src/config_api.hpp`, `src/config_manager.hpp`, or root-level helper headers. Update all includes to the new canonical paths.
2. **Use two-layer platform macros.** Public/package-root code uses project-defined macros: `ECNUVPN_PLATFORM_WINDOWS`, `ECNUVPN_PLATFORM_LINUX`, and `ECNUVPN_PLATFORM_DARWIN`. Platform implementation files may still use system/compiler macros when interacting with system headers or low-level APIs.
3. **Keep helper outside core.** `helper` is independent of `core` and belongs under `src/helper/`.
4. **Keep config inside core.** `config` is a core submodule and remains under `src/core/config/`.
5. **Avoid build-target overreach in this pass.** This pass consolidates directory ownership, include paths, CMake source lists, and platform macro boundaries. It does not need to split the project into new `exv-config` or `exv-helper` CMake targets unless required for correctness.

## 3. Target Directory Layout

### 3.1 Config Package

`config` remains under `core`:

```text
src/core/config/
  config.hpp
  config_api.hpp
  config_manager.hpp
  config_original.cpp
  config_manager.cpp
  config_api.cpp
  config_persistence.cpp
  config_show.cpp
  config_set_value.cpp
  config_routes.cpp
  config_key.cpp
  config_wizard_common.cpp
  config_wizard_ui.inc.cpp
  config_wizard_routes.inc.cpp
  config_wizard_flow.inc.cpp
  config_persistence_legacy.inc.cpp
  config_show_legacy.inc.cpp
  config_import_legacy.inc.cpp
  config_set_value_legacy.inc.cpp
  config_maintenance_legacy.inc.cpp

  platform/
    config_defaults.hpp
    darwin/
      config_defaults.cpp
    linux/
      config_defaults.cpp
    win32/
      config_defaults.cpp
```

Move the current config platform defaults as follows:

| Current path | New path |
|--------------|----------|
| `src/platform/common/config_defaults.hpp` | `src/core/config/platform/config_defaults.hpp` |
| `src/platform/darwin/config_defaults.cpp` | `src/core/config/platform/darwin/config_defaults.cpp` |
| `src/platform/common/config_defaults_linux.cpp` | `src/core/config/platform/linux/config_defaults.cpp` |
| `src/platform/win32/config_defaults.cpp` | `src/core/config/platform/win32/config_defaults.cpp` |

Delete these root compatibility headers after updating all consumers:

```text
src/config.hpp
src/config_api.hpp
src/config_manager.hpp
```

### 3.2 Helper Package

`helper` becomes a top-level module under `src/helper/`. The two `helper_client.hpp` headers have different roles: `src/helper/common/helper_client.hpp` is protocol/client-domain shared code, while `src/helper/platform/helper_client.hpp` declares the platform transport boundary used to reach the privileged helper.

`helper_main.cpp` remains the entry source for the existing `exv-helper` executable; it moves into the package but is not part of the helper runtime static-library source list.

`helper` target layout:

```text
src/helper/
  helper.hpp
  helper.cpp
  helper_internal.hpp
  helper_main.cpp
  helper_ipc.hpp
  helper_v2_handler.hpp
  helper_v2_handler.cpp

  common/
    helper_protocol.hpp
    helper_client.hpp
    pipe_helper_client.hpp
    pipe_helper_client.cpp
    helper_session_lease.hpp
    helper_capabilities.hpp
    helper_capabilities.cpp
    helper_error.hpp
    helper_messages.hpp
    helper_messages.cpp
    helper_connector.hpp
    helper_connector.cpp

  runtime/
    helper_server.hpp
    helper_server.cpp
    helper_request_dispatcher.hpp
    helper_request_dispatcher.cpp
    helper_lifecycle_policy.hpp
    helper_lifecycle_policy.cpp
    helper_session_store.hpp
    helper_session_store.cpp
    session_lease_manager.hpp
    session_lease_manager.cpp
    cleanup_registry.hpp
    cleanup_registry.cpp
    command_validator.hpp
    command_validator.cpp

  platform/
    helper_client.hpp
    helper_lifecycle.hpp
    helper_platform.hpp
    helper_service_manager.hpp
    helper_delegating_network_ops.hpp
    helper_delegating_network_ops.cpp

    darwin/
      helper_client.cpp
      helper_daemon.cpp
      helper_lifecycle.cpp
      helper_lifecycle_install.inc.cpp
      helper_lifecycle_version.inc.cpp
      helper_lifecycle_worker.inc.cpp
      helper_platform.cpp
      helper_service_manager.cpp

    linux/
      helper_client.cpp
      helper_daemon.cpp
      helper_lifecycle.cpp
      helper_platform.cpp
      helper_service_manager.cpp

    win32/
      helper_client.cpp
      helper_daemon.cpp
      helper_lifecycle.cpp
      helper_platform.cpp
      helper_service.cpp
      helper_service_manager.cpp
```

Move helper files as follows:

| Current path/pattern | New path/pattern |
|----------------------|------------------|
| `src/helper.hpp`, `src/helper.cpp`, `src/helper_internal.hpp`, `src/helper_main.cpp`, `src/helper_ipc.hpp`, `src/helper_v2_handler.*` | `src/helper/` |
| `src/helper_common/*` | `src/helper/common/*` |
| `src/helper_runtime/*` | `src/helper/runtime/*` |
| `src/helper_daemon_mac.cpp` | `src/helper/platform/darwin/helper_daemon.cpp` |
| `src/helper_daemon_linux.cpp` | `src/helper/platform/linux/helper_daemon.cpp` |
| `src/helper_daemon_win.cpp` | `src/helper/platform/win32/helper_daemon.cpp` |
| `src/helper_service_win.cpp` | `src/helper/platform/win32/helper_service.cpp` |
| `src/platform/common/helper_client.hpp` | `src/helper/platform/helper_client.hpp` |
| `src/platform/common/helper_lifecycle.hpp` | `src/helper/platform/helper_lifecycle.hpp` |
| `src/platform/common/helper_platform.hpp` | `src/helper/platform/helper_platform.hpp` |
| `src/platform/common/helper_service_manager.hpp` | `src/helper/platform/helper_service_manager.hpp` |
| `src/platform/common/helper_delegating_network_ops.*` | `src/helper/platform/helper_delegating_network_ops.*` |
| `src/platform/{darwin,linux,win32}/helper_*.cpp` | `src/helper/platform/{darwin,linux,win32}/helper_*.cpp` |

## 4. Include Rules

After consolidation, include paths should show package ownership explicitly.

### 4.1 Config Includes

Use canonical config includes:

```cpp
#include "core/config/config.hpp"
#include "core/config/config_api.hpp"
#include "core/config/config_manager.hpp"
#include "core/config/platform/config_defaults.hpp"
```

Do not use:

```cpp
#include "config.hpp"
#include "config_api.hpp"
#include "config_manager.hpp"
#include "platform/common/config_defaults.hpp"
```

### 4.2 Helper Includes

Use canonical helper includes:

```cpp
#include "helper/helper.hpp"
#include "helper/helper_ipc.hpp"
#include "helper/helper_v2_handler.hpp"
#include "helper/common/helper_messages.hpp"
#include "helper/common/helper_connector.hpp"
#include "helper/runtime/helper_server.hpp"
#include "helper/platform/helper_client.hpp"
#include "helper/platform/helper_lifecycle.hpp"
#include "helper/platform/helper_platform.hpp"
#include "helper/platform/helper_service_manager.hpp"
```

Do not use:

```cpp
#include "helper.hpp"
#include "helper_ipc.hpp"
#include "helper_v2_handler.hpp"
#include "helper_common/..."
#include "helper_runtime/..."
#include "platform/common/helper_..."
```

### 4.3 Platform Implementation Includes

Package `platform/<os>/` implementation files may include system headers and may use compiler/system macros for low-level API needs. That allowance does not apply to package-root code.

## 5. Platform Macro Design

CMake defines exactly one of the following project platform macros per supported build:

```text
ECNUVPN_PLATFORM_WINDOWS=1
ECNUVPN_PLATFORM_LINUX=1
ECNUVPN_PLATFORM_DARWIN=1
```

Package-root code and shared abstractions should use these macros when compile-time platform branching is unavoidable:

```cpp
#if defined(ECNUVPN_PLATFORM_WINDOWS)
inline constexpr std::string_view kPlatformName = "windows";
#elif defined(ECNUVPN_PLATFORM_DARWIN)
inline constexpr std::string_view kPlatformName = "darwin";
#elif defined(ECNUVPN_PLATFORM_LINUX)
inline constexpr std::string_view kPlatformName = "linux";
#else
#error "Unsupported ECNU-VPN platform"
#endif
```

Rules:

1. Public headers and package-root implementation files should prefer polymorphic/platform adapter interfaces over macros.
2. When macros are necessary outside `platform/`, use only `ECNUVPN_PLATFORM_*`.
3. Files under `src/**/platform/<os>/` may use `_WIN32`, `__linux__`, or `__APPLE__` for system-specific APIs.
4. CMake remains responsible for adding only the active platform's implementation source files.

## 6. CMake Design

Keep the current broad targets, but make package ownership visible in source lists. The current repository already has `exv-core`, `exv-helper-runtime`, executable `exv`, and executable `exv-helper` targets. This design reuses those existing targets; it does not introduce new long-lived `exv-config` or `exv-helper` libraries. In this section, snippets are illustrative but should cover every moved source in the final implementation.

Introduce package-oriented source variables:

```cmake
set(EXV_CONFIG_SOURCES
    src/core/config/config_original.cpp
    src/core/config/config_manager.cpp
    src/core/config/config_api.cpp
    src/core/config/config_persistence.cpp
    src/core/config/config_show.cpp
    src/core/config/config_set_value.cpp
    src/core/config/config_routes.cpp
    src/core/config/config_key.cpp
    src/core/config/config_wizard_common.cpp
)

set(EXV_HELPER_COMMON_SOURCES
    src/helper/common/helper_connector.cpp
    src/helper/common/pipe_helper_client.cpp
    src/helper/common/helper_messages.cpp
    src/helper/common/helper_capabilities.cpp
)

set(EXV_HELPER_PLATFORM_SHARED_SOURCES
    src/helper/platform/helper_delegating_network_ops.cpp
)

set(EXV_HELPER_RUNTIME_SOURCES
    src/helper/helper.cpp
    src/helper/helper_v2_handler.cpp
    src/helper/runtime/helper_request_dispatcher.cpp
    src/helper/runtime/helper_server.cpp
    src/helper/runtime/session_lease_manager.cpp
    src/helper/runtime/cleanup_registry.cpp
    src/helper/runtime/command_validator.cpp
    src/helper/runtime/helper_lifecycle_policy.cpp
    src/helper/runtime/helper_session_store.cpp
)
```

In each platform branch, append package-specific platform sources:

```cmake
if(WIN32)
    list(APPEND EXV_PLATFORM_DEFINITIONS ECNUVPN_PLATFORM_WINDOWS=1)
    list(APPEND EXV_CONFIG_PLATFORM_SOURCES
        src/core/config/platform/win32/config_defaults.cpp
    )
    list(APPEND EXV_HELPER_PLATFORM_SOURCES
        src/helper/platform/win32/helper_client.cpp
        src/helper/platform/win32/helper_lifecycle.cpp
        src/helper/platform/win32/helper_platform.cpp
        src/helper/platform/win32/helper_service.cpp
        src/helper/platform/win32/helper_service_manager.cpp
        src/helper/platform/win32/helper_daemon.cpp
    )
endif()
```

Apply platform definitions to targets that compile package-root code:

```cmake
target_compile_definitions(exv-core PUBLIC
    ECNUVPN_VERSION="${PROJECT_VERSION}"
    ${EXV_PLATFORM_DEFINITIONS}
)

target_compile_definitions(exv-helper-runtime PUBLIC
    ECNUVPN_VERSION="${PROJECT_VERSION}"
    ${EXV_PLATFORM_DEFINITIONS}
)
```

The final source grouping should make these relationships clear:

- `exv-core` receives config sources, config platform sources, helper common client sources used by core, helper platform shared sources, active helper platform client sources, common core sources, and non-helper platform adapters it still depends on.
- `exv-helper-runtime` receives helper root/runtime sources and links to `exv-core`.
- Executable `exv-helper` compiles `src/helper/helper_main.cpp` and links to `exv-helper-runtime`.
- Platform helper client code used by core must be compiled into a target visible to core. If it remains in `exv-core` for now, its path should still live under `src/helper/platform/` to show ownership.

## 7. Modern C++ Scope

The user's request explicitly asks to adopt current C++ features where appropriate. This pass should modernize where it directly improves package clarity, resource safety, or compile-time platform expression without broad semantic rewrites.

Recommended changes:

1. Use `inline constexpr std::string_view` for platform names, transport names, and fixed helper error codes where the existing API permits.
2. Prefer `std::filesystem::path` for new or moved path-manipulation helper functions, but avoid changing public JSON-facing structs just for style.
3. Use RAII wrappers inside platform implementation files when touching handles, file descriptors, service manager handles, or temporary resources.
4. Use `std::optional` for optional values only when it reduces sentinel string/boolean coupling.
5. Keep `nlohmann::json` serialization contracts stable.
6. Raise the C++ standard only after confirming toolchain support. C++20 is the safe default target for this repository; C++23 should not be required by this consolidation.

Do not perform unrelated algorithmic rewrites, protocol changes, helper protocol changes, or lifecycle policy changes in this pass.

## 8. Migration Strategy

1. **Move config platform defaults.** Move config platform default header/implementation files into `src/core/config/platform/` and update includes.
2. **Move helper package files.** Move root helper files, `helper_common`, and `helper_runtime` into `src/helper/`.
3. **Move helper platform files.** Move helper-specific platform interfaces and implementations from `src/platform/**` into `src/helper/platform/**`.
4. **Update include paths.** Replace all old include paths with canonical package-owned paths.
5. **Update CMake source lists.** Replace old paths with new paths and define `ECNUVPN_PLATFORM_*` per platform branch.
6. **Delete shims and empty directories.** Remove root compatibility headers and now-empty helper directories once all references are gone.
7. **Compile and fix fallout.** Build the C++ targets and resolve missing includes or source-list omissions.
8. **Run focused tests.** Run existing platform/status model tests and any available build/test commands relevant to changed code.
9. **Clean generated artifacts.** Remove temporary build output, logs, screenshots, or synced artifacts after verification.

## 9. Testing and Verification

Minimum verification after implementation:

1. Search confirms no stale includes remain:
   - `#include "config.hpp"`
   - `#include "config_api.hpp"`
   - `#include "config_manager.hpp"`
   - `#include "helper.hpp"`
   - `#include "helper_internal.hpp"`
   - `#include "helper_ipc.hpp"`
   - `#include "helper_v2_handler.hpp"`
   - `helper_common/`
   - `helper_runtime/`
   - `platform/common/helper_`
   - `platform/common/config_defaults.hpp`
2. CMake configure succeeds for the active platform.
3. Native C++ build succeeds for the active platform.
4. Existing focused tests still pass, including `platform_status_models_test` and `platform_network_ops_model_test` if available in the configured build.
5. `git status --short` contains only intended source, CMake, and documentation changes plus no generated artifacts that should be cleaned.

## 10. Non-Goals

This consolidation does not aim to:

- Redesign helper protocol V2.
- Change runtime daemon behavior.
- Change config file format or JSON schema.
- Split the build into new long-lived CMake libraries unless required to preserve link correctness.
- Move unrelated platform adapters such as crypto, process control, path utils, tunnel scripts, packet devices, or route operations.
- Introduce `platform/common/` inside packages. Shared abstractions belong directly under package root or a clearly named non-platform subdirectory.

## 11. Success Criteria

The work is complete when:

1. `helper` code is owned by `src/helper/` and helper-specific platform code is owned by `src/helper/platform/`.
2. `config` code remains owned by `src/core/config/` and config-specific platform code is owned by `src/core/config/platform/`.
3. Root compatibility headers for config/helper are gone.
4. Includes use canonical package paths.
5. Package-root code uses project platform macros or adapter interfaces rather than raw system platform macros.
6. CMake clearly selects package platform implementation files and defines `ECNUVPN_PLATFORM_*`.
7. The active platform build and focused tests pass.
8. No cleanup-first generated artifacts are left in synced directories.
