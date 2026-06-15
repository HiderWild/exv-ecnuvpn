# Helper and Config Package Consolidation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Consolidate `helper` into an independent `src/helper/` package and keep `config` as a clean `src/core/config/` submodule with package-owned platform implementations.

**Architecture:** This is a structure-preserving refactor: move files, update include paths, update CMake source lists, and add project-level platform macros without changing helper protocol, config schema, daemon behavior, or runtime semantics. `src/helper/platform/<os>/` and `src/core/config/platform/<os>/` own platform implementation details; package-root code uses canonical includes and `ECNUVPN_PLATFORM_*` macros.

**Tech Stack:** C++20, CMake, nlohmann/json, PowerShell on Windows, existing CTest targets.

---

## File Structure

### Create directories

- `src/core/config/platform/`
- `src/core/config/platform/darwin/`
- `src/core/config/platform/linux/`
- `src/core/config/platform/win32/`
- `src/helper/`
- `src/helper/common/`
- `src/helper/runtime/`
- `src/helper/platform/`
- `src/helper/platform/darwin/`
- `src/helper/platform/linux/`
- `src/helper/platform/win32/`

### Move config files

- Move `src/platform/common/config_defaults.hpp` to `src/core/config/platform/config_defaults.hpp`
- Move `src/platform/darwin/config_defaults.cpp` to `src/core/config/platform/darwin/config_defaults.cpp`
- Move `src/platform/common/config_defaults_linux.cpp` to `src/core/config/platform/linux/config_defaults.cpp`
- Move `src/platform/win32/config_defaults.cpp` to `src/core/config/platform/win32/config_defaults.cpp`
- Delete `src/config.hpp`
- Delete `src/config_api.hpp`
- Delete `src/config_manager.hpp`

### Move helper files

- Move `src/helper.hpp` to `src/helper/helper.hpp`
- Move `src/helper.cpp` to `src/helper/helper.cpp`
- Move `src/helper_internal.hpp` to `src/helper/helper_internal.hpp`
- Move `src/helper_main.cpp` to `src/helper/helper_main.cpp`
- Move `src/helper_ipc.hpp` to `src/helper/helper_ipc.hpp`
- Move `src/helper_handler.hpp` to `src/helper/helper_handler.hpp`
- Move `src/helper_handler.cpp` to `src/helper/helper_handler.cpp`
- Move `src/helper_service_win.cpp` to `src/helper/platform/win32/helper_service.cpp`
- Move `src/helper_daemon_linux.cpp` to `src/helper/platform/linux/helper_daemon.cpp`
- Move `src/helper_daemon_mac.cpp` to `src/helper/platform/darwin/helper_daemon.cpp`
- Move `src/helper_daemon_win.cpp` to `src/helper/platform/win32/helper_daemon.cpp`
- Move every file in `src/helper_common/` to `src/helper/common/`
- Move every file in `src/helper_runtime/` to `src/helper/runtime/`
- Move `src/platform/common/helper_client.hpp` to `src/helper/platform/helper_client.hpp`
- Move `src/platform/common/helper_lifecycle.hpp` to `src/helper/platform/helper_lifecycle.hpp`
- Move `src/platform/common/helper_platform.hpp` to `src/helper/platform/helper_platform.hpp`
- Move `src/platform/common/helper_service_manager.hpp` to `src/helper/platform/helper_service_manager.hpp`
- Move `src/platform/common/helper_delegating_network_ops.hpp` to `src/helper/platform/helper_delegating_network_ops.hpp`
- Move `src/platform/common/helper_delegating_network_ops.cpp` to `src/helper/platform/helper_delegating_network_ops.cpp`
- Move `src/platform/darwin/helper_client.cpp` to `src/helper/platform/darwin/helper_client.cpp`
- Move `src/platform/darwin/helper_lifecycle.cpp` to `src/helper/platform/darwin/helper_lifecycle.cpp`
- Move `src/platform/darwin/helper_lifecycle_install.inc.cpp` to `src/helper/platform/darwin/helper_lifecycle_install.inc.cpp`
- Move `src/platform/darwin/helper_lifecycle_version.inc.cpp` to `src/helper/platform/darwin/helper_lifecycle_version.inc.cpp`
- Move `src/platform/darwin/helper_lifecycle_worker.inc.cpp` to `src/helper/platform/darwin/helper_lifecycle_worker.inc.cpp`
- Move `src/platform/darwin/helper_platform.cpp` to `src/helper/platform/darwin/helper_platform.cpp`
- Move `src/platform/darwin/helper_service_manager.cpp` to `src/helper/platform/darwin/helper_service_manager.cpp`
- Move `src/platform/linux/helper_client.cpp` to `src/helper/platform/linux/helper_client.cpp`
- Move `src/platform/linux/helper_lifecycle.cpp` to `src/helper/platform/linux/helper_lifecycle.cpp`
- Move `src/platform/linux/helper_platform.cpp` to `src/helper/platform/linux/helper_platform.cpp`
- Move `src/platform/linux/helper_service_manager.cpp` to `src/helper/platform/linux/helper_service_manager.cpp`
- Move `src/platform/win32/helper_client.cpp` to `src/helper/platform/win32/helper_client.cpp`
- Move `src/platform/win32/helper_lifecycle.cpp` to `src/helper/platform/win32/helper_lifecycle.cpp`
- Move `src/platform/win32/helper_platform.cpp` to `src/helper/platform/win32/helper_platform.cpp`
- Move `src/platform/win32/helper_service_manager.cpp` to `src/helper/platform/win32/helper_service_manager.cpp`

### Modify C++ source and test includes

Update stale includes in `src/**/*.hpp`, `src/**/*.cpp`, `src/**/*.inc.cpp`, `tests/**/*.hpp`, and `tests/**/*.cpp`.

Canonical include replacements:

| Old include/path fragment | Replacement |
|---------------------------|-------------|
| `#include "config.hpp"` | `#include "core/config/config.hpp"` |
| `#include "config_api.hpp"` | `#include "core/config/config_api.hpp"` |
| `#include "config_manager.hpp"` | `#include "core/config/config_manager.hpp"` |
| `#include "helper.hpp"` | `#include "helper/helper.hpp"` |
| `#include "helper_internal.hpp"` | `#include "helper/helper_internal.hpp"` |
| `#include "helper_ipc.hpp"` | `#include "helper/helper_ipc.hpp"` |
| `#include "helper_handler.hpp"` | `#include "helper/helper_handler.hpp"` |
| `helper_common/` | `helper/common/` |
| `helper_runtime/` | `helper/runtime/` |
| `platform/common/config_defaults.hpp` | `core/config/platform/config_defaults.hpp` |
| `platform/common/helper_client.hpp` | `helper/platform/helper_client.hpp` |
| `platform/common/helper_lifecycle.hpp` | `helper/platform/helper_lifecycle.hpp` |
| `platform/common/helper_platform.hpp` | `helper/platform/helper_platform.hpp` |
| `platform/common/helper_service_manager.hpp` | `helper/platform/helper_service_manager.hpp` |
| `platform/common/helper_delegating_network_ops.hpp` | `helper/platform/helper_delegating_network_ops.hpp` |
| `../../src/helper_common/` | `helper/common/` |

### Modify CMake

- Modify `CMakeLists.txt`.
- Raise `CMAKE_CXX_STANDARD` from `17` to `20`.
- Add `EXV_PLATFORM_DEFINITIONS` and append exactly one `ECNUVPN_PLATFORM_*` macro in each platform branch.
- Replace old helper/config source paths with new package-owned paths.
- Update test target source paths that reference moved helper/config files.
- Keep existing targets: `exv-core`, `exv-helper-runtime`, `exv`, and `exv-helper`.

---

### Task 1: Record baseline stale references

**Files:**
- Read-only: `src/**`
- Read-only: `tests/**`
- Read-only: `CMakeLists.txt`

- [ ] **Step 1: Run stale include baseline**

Run from repository root in PowerShell:

```powershell
git grep -n -E '#include "(config\.hpp|config_api\.hpp|config_manager\.hpp|helper\.hpp|helper_internal\.hpp|helper_ipc\.hpp|helper_handler\.hpp|helper_common/|helper_runtime/|platform/common/helper_|platform/common/config_defaults\.hpp)' -- src tests
```

Expected: output includes existing stale references such as `src/helper.cpp`, `src/main.cpp`, `src/core/app_api/app_api.cpp`, and helper tests. This confirms the verification gate can catch the old layout.

- [ ] **Step 2: Run stale CMake path baseline**

Run:

```powershell
git grep -n -E 'src/helper_common|src/helper_runtime|src/platform/common/helper_|src/platform/common/config_defaults|src/platform/(darwin|linux|win32)/helper_|src/platform/(darwin|win32)/config_defaults|src/helper(_|\.|$)|src/helper_daemon_|src/helper_service_win' -- CMakeLists.txt
```

Expected: output includes the old CMake source paths. This confirms the CMake verification gate can catch source-list omissions.

- [ ] **Step 3: Check current working tree before moving files**

Run:

```powershell
git status --short
```

Expected: only the design/plan docs are present before implementation starts. If other user changes appear, do not overwrite them; inspect and preserve them.

- [ ] **Step 4: Commit checkpoint if commits are authorized**

Only run this commit step if the user explicitly authorizes commits for the implementation run:

```powershell
git add docs/superpowers/specs/2026-06-14-helper-config-package-design.md docs/superpowers/plans/2026-06-14-helper-config-package-consolidation-plan.md
git commit -m @'
docs: add helper config consolidation plan

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
'@
```

Expected: a documentation checkpoint commit is created. If commits are not authorized, skip this step and continue with uncommitted changes.

---

### Task 2: Move config platform files into `src/core/config/platform`

**Files:**
- Create: `src/core/config/platform/config_defaults.hpp`
- Create: `src/core/config/platform/darwin/config_defaults.cpp`
- Create: `src/core/config/platform/linux/config_defaults.cpp`
- Create: `src/core/config/platform/win32/config_defaults.cpp`
- Delete: `src/platform/common/config_defaults.hpp`
- Delete: `src/platform/common/config_defaults_linux.cpp`
- Delete: `src/platform/darwin/config_defaults.cpp`
- Delete: `src/platform/win32/config_defaults.cpp`
- Delete: `src/config.hpp`
- Delete: `src/config_api.hpp`
- Delete: `src/config_manager.hpp`
- Modify: `src/core/config/config.hpp`
- Modify: `src/core/config/config_api.hpp`
- Modify: `src/core/config/config_api.cpp`
- Modify: `src/core/config/config_manager.hpp`
- Modify: `src/core/config/config_manager.cpp`
- Modify: C++ files under `src/` and `tests/` that include root config headers

- [ ] **Step 1: Create config platform directories and move files**

Run:

```powershell
New-Item -ItemType Directory -Force "src\core\config\platform", "src\core\config\platform\darwin", "src\core\config\platform\linux", "src\core\config\platform\win32"
Move-Item "src\platform\common\config_defaults.hpp" "src\core\config\platform\config_defaults.hpp"
Move-Item "src\platform\darwin\config_defaults.cpp" "src\core\config\platform\darwin\config_defaults.cpp"
Move-Item "src\platform\common\config_defaults_linux.cpp" "src\core\config\platform\linux\config_defaults.cpp"
Move-Item "src\platform\win32\config_defaults.cpp" "src\core\config\platform\win32\config_defaults.cpp"
```

Expected: the four config platform files now live under `src/core/config/platform/`.

- [ ] **Step 2: Delete root config compatibility headers**

Run:

```powershell
Remove-Item "src\config.hpp", "src\config_api.hpp", "src\config_manager.hpp"
```

Expected: root config shims are gone.

- [ ] **Step 3: Replace config include paths**

Run:

```powershell
$files = Get-ChildItem "src", "tests" -Recurse -File -Include *.cpp,*.hpp,*.inc.cpp
$map = [ordered]@{
  '#include "config.hpp"' = '#include "core/config/config.hpp"'
  '#include "config_api.hpp"' = '#include "core/config/config_api.hpp"'
  '#include "config_manager.hpp"' = '#include "core/config/config_manager.hpp"'
  'platform/common/config_defaults.hpp' = 'core/config/platform/config_defaults.hpp'
}
foreach ($file in $files) {
  $text = Get-Content $file.FullName -Raw
  $updated = $text
  foreach ($entry in $map.GetEnumerator()) {
    $updated = $updated.Replace($entry.Key, $entry.Value)
  }
  if ($updated -ne $text) {
    Set-Content -Path $file.FullName -Value $updated -NoNewline
  }
}
```

Expected: config includes now use `core/config/...` and `core/config/platform/config_defaults.hpp`.

- [ ] **Step 4: Verify config stale include references are gone**

Run:

```powershell
git grep -n -E '#include "(config\.hpp|config_api\.hpp|config_manager\.hpp|platform/common/config_defaults\.hpp)' -- src tests
if ($LASTEXITCODE -eq 0) { throw "stale config include references remain" }
```

Expected: no output; command throws only if stale config includes remain.

- [ ] **Step 5: Commit config move if commits are authorized**

Only run this commit step if the user explicitly authorizes commits for the implementation run:

```powershell
git add src tests CMakeLists.txt
git commit -m @'
refactor: move config platform defaults into core config package

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
'@
```

Expected: a config package checkpoint commit is created. If commits are not authorized, skip this step.

---

### Task 3: Move helper root, common, runtime, and platform files into `src/helper`

**Files:**
- Create: `src/helper/helper.hpp`
- Create: `src/helper/helper.cpp`
- Create: `src/helper/helper_internal.hpp`
- Create: `src/helper/helper_main.cpp`
- Create: `src/helper/helper_ipc.hpp`
- Create: `src/helper/helper_handler.hpp`
- Create: `src/helper/helper_handler.cpp`
- Create: `src/helper/common/*`
- Create: `src/helper/runtime/*`
- Create: `src/helper/platform/*`
- Create: `src/helper/platform/darwin/*`
- Create: `src/helper/platform/linux/*`
- Create: `src/helper/platform/win32/*`
- Delete: `src/helper_common/`
- Delete: `src/helper_runtime/`
- Delete: old root helper files
- Delete: moved helper files under `src/platform/common/`, `src/platform/darwin/`, `src/platform/linux/`, and `src/platform/win32/`

- [ ] **Step 1: Create helper package directories**

Run:

```powershell
New-Item -ItemType Directory -Force "src\helper", "src\helper\common", "src\helper\runtime", "src\helper\platform", "src\helper\platform\darwin", "src\helper\platform\linux", "src\helper\platform\win32"
```

Expected: helper package directories exist.

- [ ] **Step 2: Move helper root files**

Run:

```powershell
Move-Item "src\helper.hpp" "src\helper\helper.hpp"
Move-Item "src\helper.cpp" "src\helper\helper.cpp"
Move-Item "src\helper_internal.hpp" "src\helper\helper_internal.hpp"
Move-Item "src\helper_main.cpp" "src\helper\helper_main.cpp"
Move-Item "src\helper_ipc.hpp" "src\helper\helper_ipc.hpp"
Move-Item "src\helper_handler.hpp" "src\helper\helper_handler.hpp"
Move-Item "src\helper_handler.cpp" "src\helper\helper_handler.cpp"
Move-Item "src\helper_service_win.cpp" "src\helper\platform\win32\helper_service.cpp"
Move-Item "src\helper_daemon_linux.cpp" "src\helper\platform\linux\helper_daemon.cpp"
Move-Item "src\helper_daemon_mac.cpp" "src\helper\platform\darwin\helper_daemon.cpp"
Move-Item "src\helper_daemon_win.cpp" "src\helper\platform\win32\helper_daemon.cpp"
```

Expected: no root-level `src/helper*.cpp` or `src/helper*.hpp` files remain.

- [ ] **Step 3: Move helper common files**

Run:

```powershell
Get-ChildItem "src\helper_common" -File | Move-Item -Destination "src\helper\common"
if ((Get-ChildItem "src\helper_common" -Force | Measure-Object).Count -eq 0) { Remove-Item "src\helper_common" }
```

Expected: files formerly in `src/helper_common/` now live in `src/helper/common/`, and the empty old directory is removed.

- [ ] **Step 4: Move helper runtime files**

Run:

```powershell
Get-ChildItem "src\helper_runtime" -File | Move-Item -Destination "src\helper\runtime"
if ((Get-ChildItem "src\helper_runtime" -Force | Measure-Object).Count -eq 0) { Remove-Item "src\helper_runtime" }
```

Expected: files formerly in `src/helper_runtime/` now live in `src/helper/runtime/`, and the empty old directory is removed.

- [ ] **Step 5: Move helper platform interface files**

Run:

```powershell
Move-Item "src\platform\common\helper_client.hpp" "src\helper\platform\helper_client.hpp"
Move-Item "src\platform\common\helper_lifecycle.hpp" "src\helper\platform\helper_lifecycle.hpp"
Move-Item "src\platform\common\helper_platform.hpp" "src\helper\platform\helper_platform.hpp"
Move-Item "src\platform\common\helper_service_manager.hpp" "src\helper\platform\helper_service_manager.hpp"
Move-Item "src\platform\common\helper_delegating_network_ops.hpp" "src\helper\platform\helper_delegating_network_ops.hpp"
Move-Item "src\platform\common\helper_delegating_network_ops.cpp" "src\helper\platform\helper_delegating_network_ops.cpp"
```

Expected: helper-specific platform interfaces no longer live under `src/platform/common/`.

- [ ] **Step 6: Move Darwin helper platform implementation files**

Run:

```powershell
Move-Item "src\platform\darwin\helper_client.cpp" "src\helper\platform\darwin\helper_client.cpp"
Move-Item "src\platform\darwin\helper_lifecycle.cpp" "src\helper\platform\darwin\helper_lifecycle.cpp"
Move-Item "src\platform\darwin\helper_lifecycle_install.inc.cpp" "src\helper\platform\darwin\helper_lifecycle_install.inc.cpp"
Move-Item "src\platform\darwin\helper_lifecycle_version.inc.cpp" "src\helper\platform\darwin\helper_lifecycle_version.inc.cpp"
Move-Item "src\platform\darwin\helper_lifecycle_worker.inc.cpp" "src\helper\platform\darwin\helper_lifecycle_worker.inc.cpp"
Move-Item "src\platform\darwin\helper_platform.cpp" "src\helper\platform\darwin\helper_platform.cpp"
Move-Item "src\platform\darwin\helper_service_manager.cpp" "src\helper\platform\darwin\helper_service_manager.cpp"
```

Expected: Darwin helper platform files live under `src/helper/platform/darwin/`.

- [ ] **Step 7: Move Linux helper platform implementation files**

Run:

```powershell
Move-Item "src\platform\linux\helper_client.cpp" "src\helper\platform\linux\helper_client.cpp"
Move-Item "src\platform\linux\helper_lifecycle.cpp" "src\helper\platform\linux\helper_lifecycle.cpp"
Move-Item "src\platform\linux\helper_platform.cpp" "src\helper\platform\linux\helper_platform.cpp"
Move-Item "src\platform\linux\helper_service_manager.cpp" "src\helper\platform\linux\helper_service_manager.cpp"
```

Expected: Linux helper platform files live under `src/helper/platform/linux/`.

- [ ] **Step 8: Move Windows helper platform implementation files**

Run:

```powershell
Move-Item "src\platform\win32\helper_client.cpp" "src\helper\platform\win32\helper_client.cpp"
Move-Item "src\platform\win32\helper_lifecycle.cpp" "src\helper\platform\win32\helper_lifecycle.cpp"
Move-Item "src\platform\win32\helper_platform.cpp" "src\helper\platform\win32\helper_platform.cpp"
Move-Item "src\platform\win32\helper_service_manager.cpp" "src\helper\platform\win32\helper_service_manager.cpp"
```

Expected: Windows helper platform files live under `src/helper/platform/win32/`.

- [ ] **Step 9: Replace helper include paths**

Run:

```powershell
$files = Get-ChildItem "src", "tests" -Recurse -File -Include *.cpp,*.hpp,*.inc.cpp
$map = [ordered]@{
  '#include "helper.hpp"' = '#include "helper/helper.hpp"'
  '#include "helper_internal.hpp"' = '#include "helper/helper_internal.hpp"'
  '#include "helper_ipc.hpp"' = '#include "helper/helper_ipc.hpp"'
  '#include "helper_handler.hpp"' = '#include "helper/helper_handler.hpp"'
  'helper_common/' = 'helper/common/'
  'helper_runtime/' = 'helper/runtime/'
  'platform/common/helper_client.hpp' = 'helper/platform/helper_client.hpp'
  'platform/common/helper_lifecycle.hpp' = 'helper/platform/helper_lifecycle.hpp'
  'platform/common/helper_platform.hpp' = 'helper/platform/helper_platform.hpp'
  'platform/common/helper_service_manager.hpp' = 'helper/platform/helper_service_manager.hpp'
  'platform/common/helper_delegating_network_ops.hpp' = 'helper/platform/helper_delegating_network_ops.hpp'
  '../../src/helper/common/' = 'helper/common/'
}
foreach ($file in $files) {
  $text = Get-Content $file.FullName -Raw
  $updated = $text
  foreach ($entry in $map.GetEnumerator()) {
    $updated = $updated.Replace($entry.Key, $entry.Value)
  }
  if ($updated -ne $text) {
    Set-Content -Path $file.FullName -Value $updated -NoNewline
  }
}
```

Expected: helper includes now use `helper/...` canonical paths.

- [ ] **Step 10: Verify helper stale include references are gone**

Run:

```powershell
git grep -n -E '#include "(helper\.hpp|helper_internal\.hpp|helper_ipc\.hpp|helper_handler\.hpp|helper_common/|helper_runtime/|platform/common/helper_)' -- src tests
if ($LASTEXITCODE -eq 0) { throw "stale helper include references remain" }
```

Expected: no output; command throws only if stale helper includes remain.

- [ ] **Step 11: Commit helper move if commits are authorized**

Only run this commit step if the user explicitly authorizes commits for the implementation run:

```powershell
git add src tests CMakeLists.txt
git commit -m @'
refactor: consolidate helper package layout

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
'@
```

Expected: a helper package checkpoint commit is created. If commits are not authorized, skip this step.

---

### Task 4: Add project platform macros and update helper descriptor branching

**Files:**
- Modify: `CMakeLists.txt`
- Modify: `src/helper/helper.cpp`

- [ ] **Step 1: Raise C++ standard to C++20**

In `CMakeLists.txt`, replace:

```cmake
set(CMAKE_CXX_STANDARD 17)
```

with:

```cmake
set(CMAKE_CXX_STANDARD 20)
```

Expected: the project requests C++20.

- [ ] **Step 2: Add platform definition accumulator**

In `CMakeLists.txt`, after:

```cmake
set(EXV_PLATFORM_SOURCES)
set(EXV_PLATFORM_LIBS)
```

insert:

```cmake
set(EXV_PLATFORM_DEFINITIONS)
```

Expected: platform compile definitions can be appended in the existing platform branches.

- [ ] **Step 3: Add Darwin platform macro in CMake**

In the `if(APPLE)` branch, after the comment `# CommonCrypto links implicitly on macOS`, add:

```cmake
    list(APPEND EXV_PLATFORM_DEFINITIONS ECNUVPN_PLATFORM_DARWIN=1)
```

Expected: Darwin builds define `ECNUVPN_PLATFORM_DARWIN=1`.

- [ ] **Step 4: Add Linux platform macro in CMake**

In the `elseif(UNIX AND NOT APPLE)` branch, after:

```cmake
    find_package(OpenSSL 1.1.1 REQUIRED)
```

add:

```cmake
    list(APPEND EXV_PLATFORM_DEFINITIONS ECNUVPN_PLATFORM_LINUX=1)
```

Expected: Linux builds define `ECNUVPN_PLATFORM_LINUX=1`.

- [ ] **Step 5: Add Windows platform macro in CMake**

In the `elseif(WIN32)` branch, after the Windows platform comment block and before `list(APPEND EXV_PLATFORM_LIBS ...)`, add:

```cmake
    list(APPEND EXV_PLATFORM_DEFINITIONS ECNUVPN_PLATFORM_WINDOWS=1)
```

Expected: Windows builds define `ECNUVPN_PLATFORM_WINDOWS=1`.

- [ ] **Step 6: Apply platform definitions to targets**

In `CMakeLists.txt`, replace the current `target_compile_definitions` blocks:

```cmake
target_compile_definitions(exv-core PUBLIC
    ECNUVPN_VERSION="${PROJECT_VERSION}"
)
```

with:

```cmake
target_compile_definitions(exv-core PUBLIC
    ECNUVPN_VERSION="${PROJECT_VERSION}"
    ${EXV_PLATFORM_DEFINITIONS}
)
```

and replace:

```cmake
target_compile_definitions(exv-helper-runtime PUBLIC
    ECNUVPN_VERSION="${PROJECT_VERSION}"
)
```

with:

```cmake
target_compile_definitions(exv-helper-runtime PUBLIC
    ECNUVPN_VERSION="${PROJECT_VERSION}"
    ${EXV_PLATFORM_DEFINITIONS}
)
```

Expected: both broad C++ targets receive the active project platform macro.

- [ ] **Step 7: Add string_view include in helper.cpp**

In `src/helper/helper.cpp`, add `<string_view>` to the standard includes:

```cpp
#include <chrono>
#include <iostream>
#include <sstream>
#include <string>
#include <string_view>
#include <utility>
#include <vector>
```

Expected: helper descriptor constants can use `std::string_view`.

- [ ] **Step 8: Add helper descriptor platform constants**

In `src/helper/helper.cpp`, inside the anonymous namespace after:

```cpp
volatile sig_atomic_t daemon_stop_requested = 0;
DaemonOptions active_daemon_options;
```

insert:

```cpp
#if defined(ECNUVPN_PLATFORM_WINDOWS)
inline constexpr std::string_view kHelperPlatformName = "windows";
inline constexpr std::string_view kHelperTransportName = "named-pipe";
#elif defined(ECNUVPN_PLATFORM_DARWIN)
inline constexpr std::string_view kHelperPlatformName = "darwin";
inline constexpr std::string_view kHelperTransportName = "unix-socket";
#elif defined(ECNUVPN_PLATFORM_LINUX)
inline constexpr std::string_view kHelperPlatformName = "linux";
inline constexpr std::string_view kHelperTransportName = "unix-socket";
#else
#error "Unsupported ECNU-VPN platform"
#endif
```

Expected: helper package-root code uses project macros instead of raw system macros.

- [ ] **Step 9: Replace helper descriptor raw platform macro block**

In `src/helper/helper.cpp`, replace this block inside `make_helper_descriptor()`:

```cpp
#ifdef _WIN32
                        {"platform", "windows"},
                        {"transport", "named-pipe"},
#elif defined(__APPLE__)
                        {"platform", "darwin"},
                        {"transport", "unix-socket"},
#elif defined(__linux__)
                        {"platform", "linux"},
                        {"transport", "unix-socket"},
#else
                        {"platform", "unknown"},
                        {"transport", "unknown"},
#endif
```

with:

```cpp
                        {"platform", std::string(kHelperPlatformName)},
                        {"transport", std::string(kHelperTransportName)},
```

Expected: `make_helper_descriptor()` has no `_WIN32`, `__APPLE__`, or `__linux__` branch in package-root code.

- [ ] **Step 10: Verify package-root helper.cpp has no raw system platform macro branch**

Run:

```powershell
git grep -n -E '_WIN32|__APPLE__|__linux__' -- src/helper/helper.cpp
if ($LASTEXITCODE -eq 0) { throw "raw system platform macro remains in src/helper/helper.cpp" }
```

Expected: no output.

- [ ] **Step 11: Commit platform macro update if commits are authorized**

Only run this commit step if the user explicitly authorizes commits for the implementation run:

```powershell
git add CMakeLists.txt src/helper/helper.cpp
git commit -m @'
refactor: define project platform macros for helper package code

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
'@
```

Expected: a macro boundary checkpoint commit is created. If commits are not authorized, skip this step.

---

### Task 5: Update CMake source paths for moved packages

**Files:**
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Replace moved source paths in CMakeLists.txt**

Run:

```powershell
$file = "CMakeLists.txt"
$text = Get-Content $file -Raw
$map = [ordered]@{
  'src/core/config/config_original.cpp' = 'src/core/config/config_original.cpp'
  'src/platform/common/config_defaults_linux.cpp' = 'src/core/config/platform/linux/config_defaults.cpp'
  'src/platform/darwin/config_defaults.cpp' = 'src/core/config/platform/darwin/config_defaults.cpp'
  'src/platform/win32/config_defaults.cpp' = 'src/core/config/platform/win32/config_defaults.cpp'
  'src/helper_common/helper_connector.cpp' = 'src/helper/common/helper_connector.cpp'
  'src/helper_common/pipe_helper_client.cpp' = 'src/helper/common/pipe_helper_client.cpp'
  'src/helper_common/helper_messages.cpp' = 'src/helper/common/helper_messages.cpp'
  'src/helper_common/helper_capabilities.cpp' = 'src/helper/common/helper_capabilities.cpp'
  'src/helper_runtime/helper_request_dispatcher.cpp' = 'src/helper/runtime/helper_request_dispatcher.cpp'
  'src/helper_runtime/helper_server.cpp' = 'src/helper/runtime/helper_server.cpp'
  'src/helper_runtime/session_lease_manager.cpp' = 'src/helper/runtime/session_lease_manager.cpp'
  'src/helper_runtime/cleanup_registry.cpp' = 'src/helper/runtime/cleanup_registry.cpp'
  'src/helper_runtime/command_validator.cpp' = 'src/helper/runtime/command_validator.cpp'
  'src/helper_runtime/helper_lifecycle_policy.cpp' = 'src/helper/runtime/helper_lifecycle_policy.cpp'
  'src/helper_runtime/helper_session_store.cpp' = 'src/helper/runtime/helper_session_store.cpp'
  'src/helper.cpp' = 'src/helper/helper.cpp'
  'src/helper_handler.cpp' = 'src/helper/helper_handler.cpp'
  'src/helper_main.cpp' = 'src/helper/helper_main.cpp'
  'src/helper_daemon_mac.cpp' = 'src/helper/platform/darwin/helper_daemon.cpp'
  'src/helper_daemon_linux.cpp' = 'src/helper/platform/linux/helper_daemon.cpp'
  'src/helper_daemon_win.cpp' = 'src/helper/platform/win32/helper_daemon.cpp'
  'src/helper_service_win.cpp' = 'src/helper/platform/win32/helper_service.cpp'
  'src/platform/common/helper_delegating_network_ops.cpp' = 'src/helper/platform/helper_delegating_network_ops.cpp'
  'src/platform/darwin/helper_client.cpp' = 'src/helper/platform/darwin/helper_client.cpp'
  'src/platform/darwin/helper_lifecycle.cpp' = 'src/helper/platform/darwin/helper_lifecycle.cpp'
  'src/platform/darwin/helper_platform.cpp' = 'src/helper/platform/darwin/helper_platform.cpp'
  'src/platform/darwin/helper_service_manager.cpp' = 'src/helper/platform/darwin/helper_service_manager.cpp'
  'src/platform/linux/helper_client.cpp' = 'src/helper/platform/linux/helper_client.cpp'
  'src/platform/linux/helper_lifecycle.cpp' = 'src/helper/platform/linux/helper_lifecycle.cpp'
  'src/platform/linux/helper_platform.cpp' = 'src/helper/platform/linux/helper_platform.cpp'
  'src/platform/linux/helper_service_manager.cpp' = 'src/helper/platform/linux/helper_service_manager.cpp'
  'src/platform/win32/helper_client.cpp' = 'src/helper/platform/win32/helper_client.cpp'
  'src/platform/win32/helper_lifecycle.cpp' = 'src/helper/platform/win32/helper_lifecycle.cpp'
  'src/platform/win32/helper_platform.cpp' = 'src/helper/platform/win32/helper_platform.cpp'
  'src/platform/win32/helper_service_manager.cpp' = 'src/helper/platform/win32/helper_service_manager.cpp'
}
foreach ($entry in $map.GetEnumerator()) {
  $text = $text.Replace($entry.Key, $entry.Value)
}
Set-Content -Path $file -Value $text -NoNewline
```

Expected: `CMakeLists.txt` references moved package paths.

- [ ] **Step 2: Replace Darwin helper lifecycle include file references in CMake if present**

Run:

```powershell
git grep -n -E 'src/platform/darwin/helper_lifecycle_(install|version|worker)\.inc\.cpp' -- CMakeLists.txt
if ($LASTEXITCODE -eq 0) { throw "Darwin helper lifecycle include fragments still use old paths in CMakeLists.txt" }
```

Expected: no output. The current CMake file does not list these include fragments directly; this check confirms no hidden stale path remains.

- [ ] **Step 3: Verify CMake no longer references old helper/config package paths**

Run:

```powershell
git grep -n -E 'src/helper_common|src/helper_runtime|src/platform/common/helper_|src/platform/common/config_defaults|src/platform/(darwin|linux|win32)/helper_|src/platform/(darwin|win32)/config_defaults|src/helper(_|\.|$)|src/helper_daemon_|src/helper_service_win' -- CMakeLists.txt
if ($LASTEXITCODE -eq 0) { throw "stale CMake package source paths remain" }
```

Expected: no output.

- [ ] **Step 4: Verify moved files have CMake references where required**

Run:

```powershell
$required = @(
  'src/core/config/platform/win32/config_defaults.cpp',
  'src/core/config/platform/linux/config_defaults.cpp',
  'src/core/config/platform/darwin/config_defaults.cpp',
  'src/helper/helper.cpp',
  'src/helper/helper_main.cpp',
  'src/helper/helper_handler.cpp',
  'src/helper/common/helper_messages.cpp',
  'src/helper/common/helper_connector.cpp',
  'src/helper/common/pipe_helper_client.cpp',
  'src/helper/runtime/session_lease_manager.cpp',
  'src/helper/runtime/cleanup_registry.cpp',
  'src/helper/platform/helper_delegating_network_ops.cpp',
  'src/helper/platform/win32/helper_client.cpp',
  'src/helper/platform/linux/helper_client.cpp',
  'src/helper/platform/darwin/helper_client.cpp'
)
$text = Get-Content "CMakeLists.txt" -Raw
foreach ($path in $required) {
  if (-not $text.Contains($path)) { throw "CMakeLists.txt does not reference $path" }
}
```

Expected: no output and no exception.

- [ ] **Step 5: Commit CMake path update if commits are authorized**

Only run this commit step if the user explicitly authorizes commits for the implementation run:

```powershell
git add CMakeLists.txt
git commit -m @'
build: wire consolidated helper and config package paths

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
'@
```

Expected: a CMake wiring checkpoint commit is created. If commits are not authorized, skip this step.

---

### Task 6: Configure, build, test, and clean artifacts

**Files:**
- Read: `CMakeLists.txt`
- Read: `src/**`
- Read: `tests/**`
- Generated outside repo: `$env:TEMP\ecnu-vpn-helper-config-consolidation-build`

- [ ] **Step 1: Run final stale include gate**

Run:

```powershell
git grep -n -E '#include "(config\.hpp|config_api\.hpp|config_manager\.hpp|helper\.hpp|helper_internal\.hpp|helper_ipc\.hpp|helper_handler\.hpp|helper_common/|helper_runtime/|platform/common/helper_|platform/common/config_defaults\.hpp)' -- src tests
if ($LASTEXITCODE -eq 0) { throw "stale include references remain" }
```

Expected: no output.

- [ ] **Step 2: Run final stale path gate**

Run:

```powershell
git grep -n -E 'src/helper_common|src/helper_runtime|src/platform/common/helper_|src/platform/common/config_defaults|src/platform/(darwin|linux|win32)/helper_|src/platform/(darwin|win32)/config_defaults|src/helper(_|\.|$)|src/helper_daemon_|src/helper_service_win' -- CMakeLists.txt
if ($LASTEXITCODE -eq 0) { throw "stale CMake paths remain" }
```

Expected: no output.

- [ ] **Step 3: Configure in a temporary build directory outside the repository**

Run:

```powershell
$buildDir = Join-Path $env:TEMP "ecnu-vpn-helper-config-consolidation-build"
Remove-Item -Recurse -Force $buildDir -ErrorAction SilentlyContinue
cmake -S . -B $buildDir -DCMAKE_BUILD_TYPE=Debug
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

Expected: CMake configure succeeds and writes generated files under `$env:TEMP`, not inside the synced repository.

- [ ] **Step 4: Build the application and focused test targets**

Run:

```powershell
$buildDir = Join-Path $env:TEMP "ecnu-vpn-helper-config-consolidation-build"
$targets = @(
  "exv",
  "exv-helper",
  "platform_status_models_test",
  "platform_network_ops_model_test",
  "helper_lease_manager_test",
  "helper_cleanup_registry_test",
  "helper_contract_test",
  "helper_messages_connector_test",
  "helper_delegating_network_ops_test",
  "pipe_helper_client_test"
)
foreach ($target in $targets) {
  cmake --build $buildDir --config Debug --target $target
  if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}
```

Expected: all listed targets build successfully.

- [ ] **Step 5: Run focused tests**

Run:

```powershell
$buildDir = Join-Path $env:TEMP "ecnu-vpn-helper-config-consolidation-build"
ctest --test-dir $buildDir -C Debug -R "platform_status_models_test|platform_network_ops_model_test|helper_lease_manager_test|helper_cleanup_registry_test|helper_contract_test|helper_messages_connector_test|helper_delegating_network_ops_test|pipe_helper_client_test" --output-on-failure
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
```

Expected: all focused tests pass.

- [ ] **Step 6: Inspect synced directories for generated artifacts**

Run:

```powershell
git status --short
Get-ChildItem "webui" -Force -ErrorAction SilentlyContinue | Where-Object { $_.Name -in @("node_modules", "native", "dist-electron") -or $_.Name -like "*.log" -or $_.Name -like "*.png" }
Get-ChildItem ".claude\worktrees" -Force -ErrorAction SilentlyContinue
```

Expected: `git status --short` shows only intended source, CMake, test, and docs changes. The artifact checks show no newly generated cleanup-first artifacts that need to be removed.

- [ ] **Step 7: Remove temporary build directory**

Run:

```powershell
$buildDir = Join-Path $env:TEMP "ecnu-vpn-helper-config-consolidation-build"
Remove-Item -Recurse -Force $buildDir -ErrorAction SilentlyContinue
```

Expected: temporary build output is removed.

- [ ] **Step 8: Commit final verified refactor if commits are authorized**

Only run this commit step if the user explicitly authorizes commits for the implementation run:

```powershell
git add CMakeLists.txt src tests docs/superpowers/specs/2026-06-14-helper-config-package-design.md docs/superpowers/plans/2026-06-14-helper-config-package-consolidation-plan.md
git commit -m @'
refactor: consolidate helper and config packages

Co-Authored-By: Claude Opus 4.8 (1M context) <noreply@anthropic.com>
'@
```

Expected: a final verified refactor commit is created. If earlier checkpoint commits were created, use this final commit only for remaining changes.

---

## Self-Review Checklist for the Implementer

- [ ] Every moved helper/config file has a new package-owned path.
- [ ] Root config shim headers are deleted.
- [ ] Root helper files are moved under `src/helper/`.
- [ ] `src/helper_common/` and `src/helper_runtime/` no longer exist.
- [ ] Helper-specific files no longer live under `src/platform/common/` or `src/platform/<os>/`.
- [ ] Config defaults no longer live under `src/platform/common/`, `src/platform/darwin/`, or `src/platform/win32/`.
- [ ] Package-root code uses `ECNUVPN_PLATFORM_*` rather than raw `_WIN32`, `__APPLE__`, or `__linux__` branches.
- [ ] CMake defines exactly one active `ECNUVPN_PLATFORM_*` macro per platform branch.
- [ ] Active platform configure, build, and focused tests pass.
- [ ] Generated build artifacts are removed after verification.
