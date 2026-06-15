# Utils Rewrite and Module Migration Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Rewrite the monolithic utils layer into pure C++20 utility modules plus explicit CLI and platform ownership for all side-effecting system abstractions.

**Architecture:** `src/utils` becomes pure and module-friendly; `src/cli` owns terminal presentation; `src/platform` owns filesystem, path, process, privilege, runtime-discovery, interface-stat, and Windows-specific details. The migration is staged so each phase can be tested and committed, but the final state removes the old `utils.hpp` aggregation model entirely.

**Tech Stack:** C++20, CMake 3.28 `FILE_SET CXX_MODULES`, CTest, nlohmann/json, existing platform abstraction layout.

---

## File Structure

- Create `src/utils/strings.hpp`, `src/utils/strings.cpp`, and
  `src/utils/modules/strings.cppm` for pure string helpers.
- Create `src/cli/console.hpp` and `src/cli/console.cpp` for colored output and
  console initialization.
- Create `src/platform/common/file_system.hpp/.cpp` for file existence,
  directory creation, file read, and file write.
- Create `src/platform/common/runtime_paths.hpp/.cpp` for config paths,
  effective home, runtime owner overrides, redirect files, and ownership sync.
- Create `src/platform/common/process_utils.hpp/.cpp` for command execution,
  command output, shell quoting, executable path, and privilege checks.
- Create `src/platform/common/runtime_discovery.hpp/.cpp` for bundled runtime
  and openconnect lookup.
- Create `src/platform/common/interface_stats.hpp` plus platform-specific
  `.cpp` files for traffic counters.
- Create `src/platform/win32/windows_strings.hpp/.cpp` for Windows UTF-8/wide
  conversion and system error formatting.
- Delete `src/utils.hpp`, `src/utils.cpp`, and all `src/utils_*.inc.cpp` files
  in the final cleanup phase.

## Task 1: Contract And Boundary Gates

**Files:**
- Modify: `contracts/system.contract.json`
- Modify: `contracts/generated/system_contract_snapshot.json`
- Modify: `tests/contract_manifest_test.cpp`

- [ ] **Step 1: Write the failing boundary test**

Add assertions to `tests/contract_manifest_test.cpp` that require:

```cpp
ok = expect(manifest.at("modules").contains("utils"),
            "manifest must declare modules.utils") &&
     ok;
const auto &utils = manifest.at("modules").at("utils");
ok = expect(contains(utils.at("boundary").at("accepts"), "pure string values"),
            "utils boundary must accept only pure values") &&
     ok;
ok = expect(contains(utils.at("boundary").at("rejects"), "filesystem access"),
            "utils boundary must reject filesystem access") &&
     ok;
ok = expect(!std::filesystem::exists(source_dir / "src" / "utils_platform"),
            "utils must not grow a platform subdirectory") &&
     ok;
```

- [ ] **Step 2: Verify the test fails**

Run:

```powershell
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^contract_manifest_test$"
```

Expected: FAIL because `modules.utils` is absent.

- [ ] **Step 3: Add the manifest entry**

Add `modules.utils` with this boundary:

```json
"utils": {
  "boundary": {
    "accepts": [
      "pure string values",
      "in-memory value transformations",
      "deterministic formatting helpers"
    ],
    "rejects": [
      "filesystem access",
      "process execution",
      "privilege checks",
      "runtime path ownership",
      "terminal output",
      "network interface inspection",
      "platform-specific string conversion"
    ],
    "emits": [
      "pure return values",
      "no OS side effects"
    ]
  },
  "modules": [
    "exv.utils.strings"
  ]
}
```

- [ ] **Step 4: Regenerate contracts and verify**

Run:

```powershell
python scripts/generate_contracts.py
cmake --build build --target contract_manifest_test contract_generation_check --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(contract_manifest_test|contract_generation_check)$"
```

Expected: PASS.

- [ ] **Step 5: Commit**

```powershell
git add contracts/system.contract.json contracts/generated/system_contract_snapshot.json tests/contract_manifest_test.cpp
git commit -m "test: add utils boundary contract gates"
```

## Task 2: Pure String Utilities And Module

**Files:**
- Create: `src/utils/strings.hpp`
- Create: `src/utils/strings.cpp`
- Create: `src/utils/modules/strings.cppm`
- Create: `tests/utils_strings_test.cpp`
- Create: `tests/utils_strings_module_smoke_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write failing tests**

`tests/utils_strings_test.cpp` must include:

```cpp
#include "utils/strings.hpp"

#include <iostream>
#include <string>
#include <vector>

namespace {
bool expect(bool condition, const char *message) {
  if (condition) return true;
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}
} // namespace

int main() {
  bool ok = true;
  ok = expect(exv::utils::trim(" \t hello \r\n") == "hello",
              "trim removes ASCII edge whitespace") && ok;
  ok = expect(exv::utils::trim(" \t\r\n") == "",
              "trim returns empty for all whitespace") && ok;
  const auto lines = exv::utils::split_lines(" alpha\r\n\n beta \n\t\n");
  ok = expect(lines == std::vector<std::string>{"alpha", "beta"},
              "split_lines trims and drops blank lines") && ok;
  return ok ? 0 : 1;
}
```

`tests/utils_strings_module_smoke_test.cpp` must import:

```cpp
import exv.utils.strings;

#include <iostream>
#include <string>
#include <vector>

int main() {
  bool ok = exv::utils::trim(" value ") == "value" &&
            exv::utils::split_lines("a\n\n b\r\n") ==
                std::vector<std::string>{"a", "b"};
  if (!ok) {
    std::cerr << "utils strings module smoke test failed\n";
  }
  return ok ? 0 : 1;
}
```

- [ ] **Step 2: Verify tests fail**

Run:

```powershell
cmake --build build --target utils_strings_test utils_strings_module_smoke_test --config Debug
```

Expected: FAIL because targets and files do not exist.

- [ ] **Step 3: Implement the string utility API**

Add `src/utils/strings.hpp`:

```cpp
#pragma once

#include <string>
#include <string_view>
#include <vector>

namespace exv::utils {

std::string trim(std::string_view value);
std::vector<std::string> split_lines(std::string_view text);

} // namespace exv::utils
```

Add `src/utils/strings.cpp` with deterministic implementations that use only
standard string operations.

Add `src/utils/modules/strings.cppm`:

```cpp
module;

#include "utils/strings.hpp"

export module exv.utils.strings;

export namespace exv::utils {
using ::exv::utils::split_lines;
using ::exv::utils::trim;
} // namespace exv::utils
```

- [ ] **Step 4: Register CMake targets**

Add `exv-utils-strings-module`, `utils_strings_test`, and
`utils_strings_module_smoke_test`. Link the smoke test to
`exv-utils-strings-module` and `src/utils/strings.cpp`; enable
`CXX_SCAN_FOR_MODULES ON`.

- [ ] **Step 5: Verify**

Run:

```powershell
cmake --build build --target utils_strings_test utils_strings_module_smoke_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(utils_strings_test|utils_strings_module_smoke_test)$"
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add CMakeLists.txt src/utils tests/utils_strings_test.cpp tests/utils_strings_module_smoke_test.cpp
git commit -m "feat: add pure utils strings module"
```

## Task 3: Move Terminal Output To CLI

**Files:**
- Create: `src/cli/console.hpp`
- Create: `src/cli/console.cpp`
- Create: `tests/cli_console_test.cpp`
- Modify: consumers that call `utils::print_*` or `utils::enable_windows_ansi`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write the failing boundary test**

Extend `tests/contract_manifest_test.cpp` to scan `src/utils` and fail if a
source file contains `<iostream>`, `std::cout`, or `std::cerr`.

- [ ] **Step 2: Create CLI console API**

Expose:

```cpp
namespace ecnuvpn::cli {
void print_success(const std::string &msg);
void print_error(const std::string &msg);
void print_info(const std::string &msg);
void print_warning(const std::string &msg);
void print_header(const std::string &msg);
void enable_windows_ansi();
} // namespace ecnuvpn::cli
```

- [ ] **Step 3: Migrate terminal consumers**

Replace `utils::print_*` and `utils::enable_windows_ansi` calls with
`cli::print_*` and `cli::enable_windows_ansi`. Add `#include "cli/console.hpp"`
where required.

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^contract_manifest_test$"
```

- [ ] **Step 5: Commit**

```powershell
git add CMakeLists.txt src/cli tests/contract_manifest_test.cpp src tests
git commit -m "refactor: move terminal output out of utils"
```

## Task 4: Move Filesystem And Runtime Paths To Platform

**Files:**
- Create: `src/platform/common/file_system.hpp`
- Create: `src/platform/common/file_system.cpp`
- Create: `src/platform/common/runtime_paths.hpp`
- Create: `src/platform/common/runtime_paths.cpp`
- Modify: config, runtime, logging, app API, and tests that use path or file
  functions from `utils`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write failing boundary tests**

Extend `tests/contract_manifest_test.cpp` so `src/utils` fails on
`<filesystem>`, `<fstream>`, `<sys/stat.h>`, `<direct.h>`, `_mkdir`, `mkdir`,
and `platform/common/path_utils.hpp`.

- [ ] **Step 2: Implement platform file and runtime path APIs**

Move these behaviors to platform:

- `file_exists`
- `ensure_dir`
- `read_file`
- `write_file`
- `expand_home`
- `get_config_dir`
- `set_config_dir`
- `get_config_path`
- `get_log_path`
- `get_tunnel_path`
- `get_effective_home`
- `get_home_for_uid`
- `get_username_for_uid`
- `get_config_dir_for_uid`
- runtime owner override and ownership sync
- `fix_config_dir_ownership`

- [ ] **Step 3: Migrate consumers**

Replace utils calls with `ecnuvpn::platform` calls and explicit includes:

```cpp
#include "platform/common/file_system.hpp"
#include "platform/common/runtime_paths.hpp"
```

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build build --target contract_manifest_test config_actions_test runtime_status_native_test app_api_runtime_policy_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(contract_manifest_test|config_actions_test|runtime_status_native_test|app_api_runtime_policy_test)$"
```

- [ ] **Step 5: Commit**

```powershell
git add CMakeLists.txt src/platform src/core src/runtime tests
git commit -m "refactor: move runtime paths and file IO to platform"
```

## Task 5: Move Process And Runtime Discovery To Platform

**Files:**
- Create: `src/platform/common/process_utils.hpp`
- Create: `src/platform/common/process_utils.cpp`
- Create: `src/platform/common/runtime_discovery.hpp`
- Create: `src/platform/common/runtime_discovery.cpp`
- Modify: platform/helper/openconnect consumers
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write failing boundary tests**

Extend `tests/contract_manifest_test.cpp` so `src/utils` fails on `system(`,
`popen`, `_popen`, `GetModuleFileName`, `readlink`, `_NSGetExecutablePath`,
`geteuid`, and Windows admin SID APIs.

- [ ] **Step 2: Move process APIs**

Move these behaviors to `ecnuvpn::platform`:

- `run_command`
- `run_command_output`
- `shell_quote`
- `get_executable_path`
- `check_root`

- [ ] **Step 3: Move runtime discovery APIs**

Move these behaviors to `ecnuvpn::platform`:

- `get_bundled_runtime_dir`
- `get_bundled_openconnect_path`
- `get_bundled_wintun_path`
- `get_bundled_tap_installer_path`
- `get_openconnect_path`
- `check_openconnect`

- [ ] **Step 4: Migrate consumers and verify**

Run:

```powershell
cmake --build build --target helper_contract_test helper_messages_connector_test tunnel_script_contract_test native_engine_contract_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(helper_contract_test|helper_messages_connector_test|tunnel_script_contract_test|native_engine_contract_test)$"
```

- [ ] **Step 5: Commit**

```powershell
git add CMakeLists.txt src/platform src/helper src/core src/vpn_engine tests
git commit -m "refactor: move process utilities and runtime discovery to platform"
```

## Task 6: Move Interface Stats And Windows Strings To Platform

**Files:**
- Create: `src/platform/common/interface_stats.hpp`
- Create: `src/platform/darwin/interface_stats.cpp`
- Create: `src/platform/linux/interface_stats.cpp`
- Create: `src/platform/win32/interface_stats.cpp`
- Create: `src/platform/win32/windows_strings.hpp`
- Create: `src/platform/win32/windows_strings.cpp`
- Modify: Windows platform consumers
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write failing boundary tests**

Extend `tests/contract_manifest_test.cpp` so `src/utils` fails on IP Helper,
netstat, sysfs network statistics, `MultiByteToWideChar`,
`WideCharToMultiByte`, and `FormatMessageW`.

- [ ] **Step 2: Move APIs**

Move these behaviors to platform:

- `get_interface_traffic`
- `wide_from_utf8`
- `utf8_from_wide`
- `windows_error_message`

- [ ] **Step 3: Migrate consumers and verify**

Run:

```powershell
cmake --build build --target contract_manifest_test win32_platform_network_ops_test tunnel_controller_integration_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(contract_manifest_test|win32_platform_network_ops_test|tunnel_controller_integration_test)$"
```

On non-Windows platforms, omit `win32_platform_network_ops_test`.

- [ ] **Step 4: Commit**

```powershell
git add CMakeLists.txt src/platform src tests/contract_manifest_test.cpp
git commit -m "refactor: move interface stats and Windows strings to platform"
```

## Task 7: Delete The Old Utils Aggregator

**Files:**
- Delete: `src/utils.hpp`
- Delete: `src/utils.cpp`
- Delete: `src/utils_terminal.inc.cpp`
- Delete: `src/utils_strings.inc.cpp`
- Delete: `src/utils_runtime_paths.inc.cpp`
- Delete: `src/utils_runtime_discovery.inc.cpp`
- Delete: `src/utils_process.inc.cpp`
- Delete: `src/utils_paths.inc.cpp`
- Delete: `src/utils_interface_stats.inc.cpp`
- Delete: `src/utils_file_io.inc.cpp`
- Modify: `CMakeLists.txt`
- Modify: remaining consumers

- [ ] **Step 1: Write final boundary gates**

Extend `tests/contract_manifest_test.cpp`:

```cpp
ok = expect(!std::filesystem::exists(source_dir / "src" / "utils.hpp"),
            "monolithic utils.hpp must be removed") &&
     ok;
ok = expect(!std::filesystem::exists(source_dir / "src" / "utils.cpp"),
            "monolithic utils.cpp must be removed") &&
     ok;
ok = expect(!tree_contains_matching_filename(source_dir / "src", "utils_", ".inc.cpp"),
            "utils include-unit sources must be removed") &&
     ok;
ok = expect(!tree_contains_any(source_dir / "src", {"#include \"utils.hpp\""},
                               "production code must include explicit utils, cli, or platform headers"),
            "production code must not include utils.hpp") &&
     ok;
```

- [ ] **Step 2: Delete old files and migrate final includes**

Remove all remaining `#include "utils.hpp"` consumers. Replace calls with:

- `exv::utils::*` from `utils/strings.hpp`
- `ecnuvpn::cli::*` from `cli/console.hpp`
- `ecnuvpn::platform::*` from explicit platform headers

- [ ] **Step 3: Verify no old references remain**

Run:

```powershell
rg -n '#include "utils\.hpp"' src tests
rg -n 'src/utils_' CMakeLists.txt src tests
rg -n 'utils::(print_|file_exists|ensure_dir|read_file|write_file|run_command|run_command_output|shell_quote|get_config|get_log|get_tunnel|get_effective|get_bundled|get_openconnect|check_root|get_interface_traffic|wide_from_utf8|utf8_from_wide|windows_error_message)' src tests
```

Expected: no matches.

- [ ] **Step 4: Full verification**

Run:

```powershell
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
git diff --check
```

Expected: build succeeds, all CTest tests pass, and diff check is clean.

- [ ] **Step 5: Commit**

```powershell
git add -A
git commit -m "refactor: remove monolithic utils aggregator"
```

## Notes For Future Agents

Do not reintroduce `utils.hpp` or `src/utils_*.inc.cpp`. If a new helper needs
OS state, it belongs in `src/platform`. If it writes to the user terminal, it
belongs in `src/cli`. If it is pure and platform-independent, add it under
`src/utils` and consider a narrow C++20 module facade.

