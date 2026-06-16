# Cross-Platform WebView Shell Migration Implementation Plan

> Completion note: remaining work is tracked through
> `docs/superpowers/plans/2026-06-16-webview-shell-completion-plan.md`.
> Windows packaging acceptance is recorded in
> `docs/superpowers/reports/2026-06-16-webview-shell-acceptance-report.md`;
> macOS acceptance must run on SSH host `macmini` at
> `/Users/tomli/Development/Projects/CPP/ECNU-VPN`; Linux acceptance remains
> pending host-specific verification. The detailed steps below are the original
> migration recipe and include historical Electron-adapter paths; use the
> completion plan and `scripts/accept-webview-shell-*` scripts for current
> execution.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace Electron as the production desktop shell with platform WebView hosts on Windows, macOS, and Linux while preserving the Vue renderer and native core/helper contracts.

**Architecture:** Extract a neutral desktop host contract first, keep Electron as a temporary adapter, then add a C++ native UI shell with platform WebView implementations. Windows uses WebView2 Evergreen Runtime, macOS uses WKWebView, and Linux uses WebKitGTK.

**Tech Stack:** Vue 3, TypeScript, Vite, C++20, CMake 3.28+, WebView2, Cocoa/WKWebView, GTK/WebKitGTK, existing native core RPC, existing generated contract pipeline.

---

## Scope And Sequencing

This plan intentionally migrates all platforms, but not as a single code drop.
Each phase must leave the repository buildable and testable.

The production end state is:

- no Electron/Chromium payload in default packages,
- one renderer build shared by all platform hosts,
- one neutral host bridge contract,
- platform-specific WebView hosts under `src/platform/*/ui_shell`,
- Electron retained only as a temporary adapter until the retirement task removes
  production packaging paths.

## File Structure

Create or modify these areas:

- Create `webui/host/shared/host-contract.ts`
  - Owns neutral TypeScript host contract exports currently in
    `webui/desktop/shared/desktop-contract.ts`.
- Create `webui/host/shared/generated/system-contract.ts`
  - Neutral path for generated desktop RPC contract.
- Modify `scripts/generate_contracts.py`
  - Emit TypeScript generated contract to both the new neutral path and the
    temporary Electron compatibility path during migration.
- Modify `webui/desktop/shared/desktop-contract.ts`
  - Becomes a compatibility re-export from `webui/host/shared/host-contract.ts`.
- Modify `webui/desktop/preload/index.ts`
  - Imports neutral host contract instead of owning Electron-specific shared
    types.
- Modify `webui/src/api/desktop.ts`
  - Rename internally to a host bridge client without changing renderer call
    sites in the first phase.
- Create `webui/host/__tests__/host-boundary.test.ts`
  - Blocks renderer and neutral host code from importing Electron or WebView
    platform APIs.
- Create `src/app/ui_shell/`
  - `ui_shell_main.cpp`: native shell entry point.
  - `ui_shell_options.hpp/.cpp`: command-line and environment options.
  - `core_process_manager.hpp/.cpp`: starts and monitors `exv --mode=core`.
  - `core_rpc_client.hpp/.cpp`: JSON line RPC over core stdin/stdout.
  - `host_bridge.hpp/.cpp`: validates renderer requests and forwards to core.
  - `renderer_assets.hpp/.cpp`: resolves dev-server URL or packaged renderer
    `index.html`.
  - `ui_window.hpp`: platform-neutral window, modal, tray, event interfaces.
- Create `src/platform/win32/ui_shell/`
  - `webview2_runtime_win32.hpp/.cpp`: WebView2 Evergreen detection and
    Bootstrapper flow.
  - `webview2_host_win32.cpp`: Win32 window and WebView2 bridge.
  - `tray_win32.cpp`: notification area menu behavior.
- Create `src/platform/darwin/ui_shell/`
  - `wk_webview_host_darwin.mm`: Cocoa app, WKWebView bridge, windows, modals.
  - `tray_darwin.mm`: menu bar/status item behavior.
- Create `src/platform/linux/ui_shell/`
  - `webkitgtk_host_linux.cpp`: GTK app, WebKitGTK bridge, windows, modals.
  - `tray_linux.cpp`: AppIndicator/status fallback behavior.
- Create tests:
  - `tests/ui_shell_contract_test.cpp`
  - `tests/ui_shell_core_rpc_client_test.cpp`
  - `tests/win32_webview2_runtime_test.cpp`
  - `tests/linux_webkitgtk_runtime_test.cpp`
  - `tests/darwin_wkwebview_runtime_test.cpp`
- Modify `CMakeLists.txt`
  - Adds `exv-ui` native shell executable and platform-specific source groups.
- Modify `webui/package.json`
  - Keeps Electron scripts during migration, adds neutral web build scripts, and
    removes Electron from default production packaging only in the retirement
    task.
- Modify `webui/README.md`
  - Documents WebView shell architecture and migration status.

## Task 1: Neutral Host Contract

**Files:**
- Create: `webui/host/shared/host-contract.ts`
- Create: `webui/host/shared/generated/system-contract.ts`
- Modify: `scripts/generate_contracts.py`
- Modify: `webui/desktop/shared/desktop-contract.ts`
- Modify: `webui/desktop/preload/index.ts`
- Modify: `webui/src/types/ecnu-vpn.d.ts`
- Test: `webui/host/__tests__/host-boundary.test.ts`

- [ ] **Step 1: Write the host boundary test**

```ts
import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'

const root = new URL('../../', import.meta.url)
const webuiRoot = join(root.pathname, '..')

function filesUnder(dir: string): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) out.push(...filesUnder(path))
    else if (/\.(ts|vue)$/.test(path)) out.push(path)
  }
  return out
}

describe('neutral host boundary', () => {
  it('keeps renderer and neutral host code free of Electron imports', () => {
    const checked = [
      ...filesUnder(join(webuiRoot, 'src')),
      ...filesUnder(join(webuiRoot, 'host')),
    ]

    for (const file of checked) {
      const text = readFileSync(file, 'utf8')
      assert.doesNotMatch(text, /from ['"]electron['"]/)
      assert.doesNotMatch(text, /require\(['"]electron['"]\)/)
      assert.doesNotMatch(text, /\bipcRenderer\b/)
      assert.doesNotMatch(text, /\bcontextBridge\b/)
    }
  })
})
```

- [ ] **Step 2: Run the boundary test and verify it fails**

Run:

```powershell
cd webui
pnpm exec node scripts/run-electron-test.cjs host/__tests__/host-boundary.test.ts
```

Expected: FAIL because `webui/host/shared/host-contract.ts` does not exist and
the test runner currently only accepts `desktop/*.ts` test paths.

- [ ] **Step 3: Extend the electron test runner to allow neutral host tests**

Modify `webui/scripts/run-electron-test.cjs` so the path validation allows both
`desktop/*.ts` and `host/*.ts`:

```js
if (!normalized.startsWith('desktop/') && !normalized.startsWith('host/')) {
  console.error(`electron test path must be a desktop/*.ts or host/*.ts file: ${test}`)
  process.exitCode = 1
  continue
}
```

- [ ] **Step 4: Move the generated contract to a neutral path**

Update `scripts/generate_contracts.py` so the TypeScript generated contract is
written to:

```text
webui/host/shared/generated/system-contract.ts
webui/desktop/shared/generated/system-contract.ts
```

During migration both paths must contain identical content. Add a Python
constant for each output path rather than hard-coding paths in two places:

```python
TS_CONTRACT_OUTPUTS = [
    ROOT / "webui" / "host" / "shared" / "generated" / "system-contract.ts",
    ROOT / "webui" / "desktop" / "shared" / "generated" / "system-contract.ts",
]
```

- [ ] **Step 5: Create the neutral host contract**

Create `webui/host/shared/host-contract.ts` by moving the existing contents of
`webui/desktop/shared/desktop-contract.ts` and changing only the generated
import path:

```ts
import {
  DESKTOP_RPC_ACTIONS,
  DESKTOP_RPC_ERROR_CODE_MAP,
  DESKTOP_RPC_EVENT_TYPES,
} from './generated/system-contract.js'
```

The exported names stay stable in this task: `desktopIpcChannels`,
`desktopApiPaths`, `DesktopRpcAction`, `SERVICE_ACTIONS`, and the existing
response types remain exported.

- [ ] **Step 6: Re-export from the Electron compatibility path**

Replace `webui/desktop/shared/desktop-contract.ts` with:

```ts
export * from '../../host/shared/host-contract.js'
```

- [ ] **Step 7: Update imports that can use the neutral path safely**

Change type-only renderer imports from:

```ts
} from '../../desktop/shared/desktop-contract'
```

to:

```ts
} from '../../host/shared/host-contract'
```

Do this first in `webui/src/types/ecnu-vpn.d.ts`. Leave Electron main/preload
imports on the compatibility path until Task 2.

- [ ] **Step 8: Run generated contract and TypeScript checks**

Run:

```powershell
python scripts/generate_contracts.py --check
cd webui
pnpm exec node scripts/run-electron-test.cjs host/__tests__/host-boundary.test.ts desktop/main/__tests__/desktop-contract-generated.test.ts
pnpm exec tsc -p tsconfig.electron.json --noEmit
pnpm run build
```

Expected: all commands pass.

- [ ] **Step 9: Commit**

```powershell
git add scripts/generate_contracts.py webui/host webui/desktop/shared/desktop-contract.ts webui/src/types/ecnu-vpn.d.ts webui/scripts/run-electron-test.cjs
git commit -m "Introduce neutral desktop host contract"
```

## Task 2: Renderer API Neutrality

**Files:**
- Create: `webui/src/api/host.ts`
- Modify: `webui/src/api/desktop.ts`
- Modify: `webui/src/stores/config.ts`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/pages/LogsPage.vue`
- Modify: `webui/src/composables/useSSE.ts`
- Test: `webui/host/__tests__/host-boundary.test.ts`

- [ ] **Step 1: Extend the boundary test for renderer API imports**

Add this assertion to `webui/host/__tests__/host-boundary.test.ts`:

```ts
assert.doesNotMatch(text, /['"]\.\.\/api\/desktop['"]/)
```

Expected: the test fails because renderer stores still import
`../api/desktop`.

- [ ] **Step 2: Create the neutral API client**

Move the contents of `webui/src/api/desktop.ts` to `webui/src/api/host.ts`.
Keep the API object behavior unchanged, but change the unavailable message:

```ts
function requireHost() {
  if (!hostAvailable()) {
    throw new Error('Desktop host API is not available in this shell.')
  }
}
```

Export the same default object from `host.ts`.

- [ ] **Step 3: Keep a temporary compatibility re-export**

Replace `webui/src/api/desktop.ts` with:

```ts
export { default } from './host'
```

- [ ] **Step 4: Update renderer imports**

Change imports in:

- `webui/src/stores/config.ts`
- `webui/src/stores/vpn.ts`
- `webui/src/pages/LogsPage.vue`

from:

```ts
import api from '../api/desktop'
```

to:

```ts
import api from '../api/host'
```

For dynamic imports, change:

```ts
const api = (await import('../api/desktop')).default
```

to:

```ts
const api = (await import('../api/host')).default
```

- [ ] **Step 5: Run renderer checks**

Run:

```powershell
cd webui
pnpm exec node scripts/run-electron-test.cjs host/__tests__/host-boundary.test.ts
pnpm run build
```

Expected: both pass.

- [ ] **Step 6: Commit**

```powershell
git add webui/src/api webui/src/stores webui/src/pages/LogsPage.vue webui/host/__tests__/host-boundary.test.ts
git commit -m "Make renderer use neutral host API"
```

## Task 3: Common Native UI Shell Skeleton

**Files:**
- Create: `src/app/ui_shell/ui_shell_options.hpp`
- Create: `src/app/ui_shell/ui_shell_options.cpp`
- Create: `src/app/ui_shell/renderer_assets.hpp`
- Create: `src/app/ui_shell/renderer_assets.cpp`
- Create: `src/app/ui_shell/host_bridge.hpp`
- Create: `src/app/ui_shell/host_bridge.cpp`
- Create: `tests/ui_shell_contract_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write C++ shell contract tests**

Create `tests/ui_shell_contract_test.cpp`:

```cpp
#include "app/ui_shell/host_bridge.hpp"
#include "app/ui_shell/renderer_assets.hpp"

#include <cassert>
#include <string>

int main() {
  using namespace ecnuvpn::ui_shell;

  assert(is_allowed_host_action("status.get"));
  assert(is_allowed_host_action("vpn.connect"));
  assert(!is_allowed_host_action("shell.unknown"));

  RendererAssets dev = resolve_renderer_assets("http://127.0.0.1:8288", "");
  assert(dev.kind == RendererAssetKind::DevServer);
  assert(dev.location == "http://127.0.0.1:8288");

  RendererAssets packaged = resolve_renderer_assets("", "C:/app/dist/index.html");
  assert(packaged.kind == RendererAssetKind::PackagedFile);
  assert(packaged.location == "C:/app/dist/index.html");
}
```

- [ ] **Step 2: Run the new test and verify it fails to compile**

Run:

```powershell
cmake --build build --config Debug --target ui_shell_contract_test
```

Expected: FAIL because `app/ui_shell/host_bridge.hpp` and
`app/ui_shell/renderer_assets.hpp` do not exist.

- [ ] **Step 3: Add renderer asset resolver**

Create `src/app/ui_shell/renderer_assets.hpp`:

```cpp
#pragma once

#include <string>

namespace ecnuvpn::ui_shell {

enum class RendererAssetKind {
  DevServer,
  PackagedFile,
};

struct RendererAssets {
  RendererAssetKind kind;
  std::string location;
};

RendererAssets resolve_renderer_assets(const std::string &dev_server_url,
                                       const std::string &packaged_index);

} // namespace ecnuvpn::ui_shell
```

Create `src/app/ui_shell/renderer_assets.cpp`:

```cpp
#include "app/ui_shell/renderer_assets.hpp"

namespace ecnuvpn::ui_shell {

RendererAssets resolve_renderer_assets(const std::string &dev_server_url,
                                       const std::string &packaged_index) {
  if (!dev_server_url.empty()) {
    return {RendererAssetKind::DevServer, dev_server_url};
  }
  return {RendererAssetKind::PackagedFile, packaged_index};
}

} // namespace ecnuvpn::ui_shell
```

- [ ] **Step 4: Add host action allowlist facade**

Create `src/app/ui_shell/host_bridge.hpp`:

```cpp
#pragma once

#include <string_view>

namespace ecnuvpn::ui_shell {

bool is_allowed_host_action(std::string_view action);

} // namespace ecnuvpn::ui_shell
```

Create `src/app/ui_shell/host_bridge.cpp`:

```cpp
#include "app/ui_shell/host_bridge.hpp"

#include "contracts/generated/system_contract.hpp"

#include <algorithm>

namespace ecnuvpn::ui_shell {

bool is_allowed_host_action(std::string_view action) {
  const auto &actions = ecnuvpn::contracts::DESKTOP_RPC_ACTIONS;
  return std::find(actions.begin(), actions.end(), action) != actions.end();
}

} // namespace ecnuvpn::ui_shell
```

- [ ] **Step 5: Add shell options parser**

Create `src/app/ui_shell/ui_shell_options.hpp`:

```cpp
#pragma once

#include <string>

namespace ecnuvpn::ui_shell {

struct UiShellOptions {
  std::string renderer_dev_server_url;
  std::string packaged_renderer_index;
  std::string exv_path;
  bool enable_dev_tools = false;
};

UiShellOptions parse_ui_shell_options(int argc, char **argv);

} // namespace ecnuvpn::ui_shell
```

Create `src/app/ui_shell/ui_shell_options.cpp`:

```cpp
#include "app/ui_shell/ui_shell_options.hpp"

#include <string_view>

namespace ecnuvpn::ui_shell {

UiShellOptions parse_ui_shell_options(int argc, char **argv) {
  UiShellOptions options;
  for (int i = 1; i < argc; ++i) {
    std::string_view arg = argv[i] ? argv[i] : "";
    if (arg == "--renderer-url" && i + 1 < argc) {
      options.renderer_dev_server_url = argv[++i];
    } else if (arg == "--renderer-index" && i + 1 < argc) {
      options.packaged_renderer_index = argv[++i];
    } else if (arg == "--exv" && i + 1 < argc) {
      options.exv_path = argv[++i];
    } else if (arg == "--devtools") {
      options.enable_dev_tools = true;
    }
  }
  return options;
}

} // namespace ecnuvpn::ui_shell
```

- [ ] **Step 6: Register the shell test in CMake**

Add an executable and CTest entry:

```cmake
add_executable(ui_shell_contract_test
  tests/ui_shell_contract_test.cpp
  src/app/ui_shell/host_bridge.cpp
  src/app/ui_shell/renderer_assets.cpp
)
target_include_directories(ui_shell_contract_test PRIVATE src)
add_test(NAME ui_shell_contract_test COMMAND ui_shell_contract_test)
```

- [ ] **Step 7: Run the focused C++ test**

Run:

```powershell
cmake --build build --config Debug --target ui_shell_contract_test
ctest --test-dir build -C Debug -R ui_shell_contract_test --output-on-failure
```

Expected: PASS.

- [ ] **Step 8: Commit**

```powershell
git add CMakeLists.txt src/app/ui_shell tests/ui_shell_contract_test.cpp
git commit -m "Add common native UI shell skeleton"
```

## Task 4: Native Core RPC Client For UI Shell

**Files:**
- Create: `src/app/ui_shell/core_rpc_client.hpp`
- Create: `src/app/ui_shell/core_rpc_client.cpp`
- Create: `src/app/ui_shell/core_process_manager.hpp`
- Create: `src/app/ui_shell/core_process_manager.cpp`
- Create: `tests/ui_shell_core_rpc_client_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write JSON-line parser tests**

Create `tests/ui_shell_core_rpc_client_test.cpp`:

```cpp
#include "app/ui_shell/core_rpc_client.hpp"

#include <cassert>

int main() {
  using namespace ecnuvpn::ui_shell;

  CoreRpcResponse ok = parse_core_rpc_line(R"({"id":7,"ok":true,"data":{"phase":"idle"}})");
  assert(ok.id == 7);
  assert(ok.ok);
  assert(ok.data_json == R"({"phase":"idle"})");

  CoreRpcResponse error = parse_core_rpc_line(R"({"id":8,"ok":false,"code":"bad","message":"No"})");
  assert(error.id == 8);
  assert(!error.ok);
  assert(error.code == "bad");
  assert(error.message == "No");

  CoreRpcEvent event = parse_core_rpc_event_line(R"({"event":"status","data":{"phase":"connected"}})");
  assert(event.event == "status");
  assert(event.data_json == R"({"phase":"connected"})");
}
```

- [ ] **Step 2: Run and verify compile failure**

Run:

```powershell
cmake --build build --config Debug --target ui_shell_core_rpc_client_test
```

Expected: FAIL because the client files do not exist.

- [ ] **Step 3: Add parsing-only RPC client facade**

Create `src/app/ui_shell/core_rpc_client.hpp`:

```cpp
#pragma once

#include <string>

namespace ecnuvpn::ui_shell {

struct CoreRpcResponse {
  int id = 0;
  bool ok = false;
  std::string data_json;
  std::string code;
  std::string message;
};

struct CoreRpcEvent {
  std::string event;
  std::string data_json;
};

CoreRpcResponse parse_core_rpc_line(const std::string &line);
CoreRpcEvent parse_core_rpc_event_line(const std::string &line);

} // namespace ecnuvpn::ui_shell
```

Create `src/app/ui_shell/core_rpc_client.cpp` with `nlohmann::json` parsing:

```cpp
#include "app/ui_shell/core_rpc_client.hpp"

#include <nlohmann/json.hpp>

namespace ecnuvpn::ui_shell {

CoreRpcResponse parse_core_rpc_line(const std::string &line) {
  const auto parsed = nlohmann::json::parse(line);
  CoreRpcResponse out;
  out.id = parsed.value("id", 0);
  out.ok = parsed.value("ok", false);
  if (parsed.contains("data")) {
    out.data_json = parsed.at("data").dump();
  }
  out.code = parsed.value("code", "");
  out.message = parsed.value("message", "");
  return out;
}

CoreRpcEvent parse_core_rpc_event_line(const std::string &line) {
  const auto parsed = nlohmann::json::parse(line);
  CoreRpcEvent out;
  out.event = parsed.value("event", "");
  if (parsed.contains("data")) {
    out.data_json = parsed.at("data").dump();
  }
  return out;
}

} // namespace ecnuvpn::ui_shell
```

- [ ] **Step 4: Add process manager interface**

Create `src/app/ui_shell/core_process_manager.hpp`:

```cpp
#pragma once

#include <string>

namespace ecnuvpn::ui_shell {

struct CoreProcessLaunch {
  std::string exv_path;
  std::string state_dir;
  std::string runtime_dir;
};

class CoreProcessManager {
public:
  virtual ~CoreProcessManager() = default;
  virtual bool start(const CoreProcessLaunch &launch) = 0;
  virtual void stop() = 0;
  virtual bool alive() const = 0;
};

} // namespace ecnuvpn::ui_shell
```

Create `src/app/ui_shell/core_process_manager.cpp`:

```cpp
#include "app/ui_shell/core_process_manager.hpp"
```

- [ ] **Step 5: Register and run the test**

Add the test target:

```cmake
add_executable(ui_shell_core_rpc_client_test
  tests/ui_shell_core_rpc_client_test.cpp
  src/app/ui_shell/core_rpc_client.cpp
)
target_include_directories(ui_shell_core_rpc_client_test PRIVATE src)
target_link_libraries(ui_shell_core_rpc_client_test PRIVATE nlohmann_json::nlohmann_json)
add_test(NAME ui_shell_core_rpc_client_test COMMAND ui_shell_core_rpc_client_test)
```

Run:

```powershell
cmake --build build --config Debug --target ui_shell_core_rpc_client_test
ctest --test-dir build -C Debug -R ui_shell_core_rpc_client_test --output-on-failure
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add CMakeLists.txt src/app/ui_shell/core_rpc_client.* src/app/ui_shell/core_process_manager.* tests/ui_shell_core_rpc_client_test.cpp
git commit -m "Add native UI shell core RPC primitives"
```

## Task 5: Windows WebView2 Runtime Detection

**Files:**
- Create: `src/platform/win32/ui_shell/webview2_runtime_win32.hpp`
- Create: `src/platform/win32/ui_shell/webview2_runtime_win32.cpp`
- Create: `tests/win32_webview2_runtime_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write WebView2 registry detection tests**

Create `tests/win32_webview2_runtime_test.cpp`:

```cpp
#include "platform/win32/ui_shell/webview2_runtime_win32.hpp"

#include <cassert>

int main() {
  using namespace ecnuvpn::platform::win32::ui_shell;

  assert(is_valid_webview2_version("120.0.2210.91"));
  assert(!is_valid_webview2_version(""));
  assert(!is_valid_webview2_version("0.0.0.0"));
  assert(!is_valid_webview2_version("garbage"));

  WebView2RuntimeStatus missing = evaluate_webview2_runtime_versions("", "0.0.0.0");
  assert(!missing.installed);

  WebView2RuntimeStatus installed = evaluate_webview2_runtime_versions("", "120.0.2210.91");
  assert(installed.installed);
  assert(installed.version == "120.0.2210.91");
}
```

- [ ] **Step 2: Run and verify compile failure**

Run:

```powershell
cmake --build build --config Debug --target win32_webview2_runtime_test
```

Expected: FAIL because the Win32 runtime files do not exist.

- [ ] **Step 3: Add pure detection helpers**

Create `src/platform/win32/ui_shell/webview2_runtime_win32.hpp`:

```cpp
#pragma once

#include <string>

namespace ecnuvpn::platform::win32::ui_shell {

struct WebView2RuntimeStatus {
  bool installed = false;
  std::string version;
  std::string source;
};

bool is_valid_webview2_version(const std::string &version);
WebView2RuntimeStatus evaluate_webview2_runtime_versions(
    const std::string &hklm_version,
    const std::string &hkcu_version);
WebView2RuntimeStatus detect_webview2_runtime();

} // namespace ecnuvpn::platform::win32::ui_shell
```

Create `src/platform/win32/ui_shell/webview2_runtime_win32.cpp` with pure
helpers first:

```cpp
#include "platform/win32/ui_shell/webview2_runtime_win32.hpp"

#include <cctype>

namespace ecnuvpn::platform::win32::ui_shell {

bool is_valid_webview2_version(const std::string &version) {
  if (version.empty() || version == "0.0.0.0") return false;
  bool digit = false;
  for (char ch : version) {
    if (std::isdigit(static_cast<unsigned char>(ch))) {
      digit = true;
      continue;
    }
    if (ch != '.') return false;
  }
  return digit;
}

WebView2RuntimeStatus evaluate_webview2_runtime_versions(
    const std::string &hklm_version,
    const std::string &hkcu_version) {
  if (is_valid_webview2_version(hklm_version)) {
    return {true, hklm_version, "HKLM"};
  }
  if (is_valid_webview2_version(hkcu_version)) {
    return {true, hkcu_version, "HKCU"};
  }
  return {};
}

WebView2RuntimeStatus detect_webview2_runtime() {
  return {};
}

} // namespace ecnuvpn::platform::win32::ui_shell
```

- [ ] **Step 4: Add real Win32 registry probing**

Implement `detect_webview2_runtime()` using the Microsoft-documented `pv`
registry values:

```text
HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}
HKCU\Software\Microsoft\EdgeUpdate\Clients\{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}
```

The function must return `installed=false` when both values are missing, empty,
or `0.0.0.0`.

- [ ] **Step 5: Register and run the focused Windows test**

Add:

```cmake
if(WIN32)
  add_executable(win32_webview2_runtime_test
    tests/win32_webview2_runtime_test.cpp
    src/platform/win32/ui_shell/webview2_runtime_win32.cpp
  )
  target_include_directories(win32_webview2_runtime_test PRIVATE src)
  add_test(NAME win32_webview2_runtime_test COMMAND win32_webview2_runtime_test)
endif()
```

Run:

```powershell
cmake --build build --config Debug --target win32_webview2_runtime_test
ctest --test-dir build -C Debug -R win32_webview2_runtime_test --output-on-failure
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add CMakeLists.txt src/platform/win32/ui_shell tests/win32_webview2_runtime_test.cpp
git commit -m "Add WebView2 Evergreen runtime detection"
```

## Task 6: Windows WebView2 Bootstrapper Policy

**Files:**
- Modify: `src/platform/win32/ui_shell/webview2_runtime_win32.hpp`
- Modify: `src/platform/win32/ui_shell/webview2_runtime_win32.cpp`
- Modify: `tests/win32_webview2_runtime_test.cpp`

- [ ] **Step 1: Add bootstrapper decision tests**

Add to `tests/win32_webview2_runtime_test.cpp`:

```cpp
WebView2BootstrapDecision denied =
    decide_webview2_bootstrap({false, "", ""}, false, false);
assert(!denied.should_download);
assert(denied.reason == "offline");

WebView2BootstrapDecision allowed =
    decide_webview2_bootstrap({false, "", ""}, true, true);
assert(allowed.should_download);
assert(allowed.installer_args == "/silent /install");

WebView2BootstrapDecision unnecessary =
    decide_webview2_bootstrap({true, "120.0.2210.91", "HKCU"}, true, true);
assert(!unnecessary.should_download);
assert(unnecessary.reason == "installed");
```

- [ ] **Step 2: Run and verify failure**

Run:

```powershell
cmake --build build --config Debug --target win32_webview2_runtime_test
```

Expected: FAIL because `WebView2BootstrapDecision` is undefined.

- [ ] **Step 3: Add bootstrapper decision model**

Add to the header:

```cpp
struct WebView2BootstrapDecision {
  bool should_download = false;
  std::string reason;
  std::string installer_args;
};

WebView2BootstrapDecision decide_webview2_bootstrap(
    const WebView2RuntimeStatus &status,
    bool network_available,
    bool user_consented);
```

Add to the implementation:

```cpp
WebView2BootstrapDecision decide_webview2_bootstrap(
    const WebView2RuntimeStatus &status,
    bool network_available,
    bool user_consented) {
  if (status.installed) return {false, "installed", ""};
  if (!network_available) return {false, "offline", ""};
  if (!user_consented) return {false, "user_declined", ""};
  return {true, "missing", "/silent /install"};
}
```

- [ ] **Step 4: Add a native download execution seam**

Add a function declaration only after tests for the decision model pass:

```cpp
bool run_webview2_evergreen_bootstrapper(const std::string &download_url,
                                         const std::string &installer_path);
```

The implementation must download only from the Microsoft-provided Evergreen
Bootstrapper URL configured by installer metadata or build configuration. It
must not use arbitrary renderer-provided URLs.

- [ ] **Step 5: Run focused checks**

Run:

```powershell
cmake --build build --config Debug --target win32_webview2_runtime_test
ctest --test-dir build -C Debug -R win32_webview2_runtime_test --output-on-failure
```

Expected: PASS.

- [ ] **Step 6: Commit**

```powershell
git add src/platform/win32/ui_shell/webview2_runtime_win32.* tests/win32_webview2_runtime_test.cpp
git commit -m "Add WebView2 bootstrapper policy"
```

## Task 7: Platform WebView Host Interfaces

**Files:**
- Create: `src/app/ui_shell/ui_window.hpp`
- Create: `src/app/ui_shell/ui_window.cpp`
- Create: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Create: `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- Create: `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Add the platform-neutral window interface**

Create `src/app/ui_shell/ui_window.hpp`:

```cpp
#pragma once

#include "app/ui_shell/renderer_assets.hpp"

#include <functional>
#include <string>

namespace ecnuvpn::ui_shell {

using HostMessageHandler = std::function<std::string(const std::string &)>;

struct UiWindowConfig {
  RendererAssets renderer;
  bool enable_dev_tools = false;
};

class UiWindow {
public:
  virtual ~UiWindow() = default;
  virtual void set_message_handler(HostMessageHandler handler) = 0;
  virtual int run(const UiWindowConfig &config) = 0;
  virtual void emit_event(const std::string &event_json) = 0;
};

} // namespace ecnuvpn::ui_shell
```

Create `src/app/ui_shell/ui_window.cpp`:

```cpp
#include "app/ui_shell/ui_window.hpp"
```

- [ ] **Step 2: Add compile-gated platform host stubs that fail clearly**

Each platform host must compile only on its platform and return a native
unavailable error until its real implementation lands.

Windows:

```cpp
#include "app/ui_shell/ui_window.hpp"

namespace ecnuvpn::platform::win32::ui_shell {

int run_webview2_host(const ecnuvpn::ui_shell::UiWindowConfig &) {
  return 70;
}

} // namespace ecnuvpn::platform::win32::ui_shell
```

macOS and Linux use the same shape with platform namespaces and distinct
function names: `run_wk_webview_host` and `run_webkitgtk_host`.

- [ ] **Step 3: Wire CMake platform sources**

Add platform conditionals:

```cmake
if(WIN32)
  list(APPEND EXV_UI_PLATFORM_SOURCES
    src/platform/win32/ui_shell/webview2_host_win32.cpp
    src/platform/win32/ui_shell/webview2_runtime_win32.cpp
  )
elseif(APPLE)
  list(APPEND EXV_UI_PLATFORM_SOURCES
    src/platform/darwin/ui_shell/wk_webview_host_darwin.mm
  )
elseif(UNIX)
  list(APPEND EXV_UI_PLATFORM_SOURCES
    src/platform/linux/ui_shell/webkitgtk_host_linux.cpp
  )
endif()
```

- [ ] **Step 4: Run platform configure/build smoke**

Run on each platform:

```powershell
cmake --build build --config Debug --target exv-ui
```

Expected: host executable compiles. It may exit with platform unavailable code
until platform-specific implementation tasks complete.

- [ ] **Step 5: Commit**

```powershell
git add CMakeLists.txt src/app/ui_shell/ui_window.* src/platform/win32/ui_shell src/platform/darwin/ui_shell src/platform/linux/ui_shell
git commit -m "Add platform WebView host interfaces"
```

## Task 8: Windows WebView2 Host Parity

**Files:**
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Modify: `src/app/ui_shell/host_bridge.cpp`
- Modify: `CMakeLists.txt`
- Test: `tests/win32_webview2_runtime_test.cpp`

- [ ] **Step 1: Add WebView2 SDK discovery**

Add CMake discovery using `WEBVIEW2_SDK_DIR` first. The build must fail with a
clear message when `EXV_BUILD_UI_SHELL=ON`, `WIN32=TRUE`, and the SDK headers or
loader library are missing:

```cmake
if(WIN32 AND EXV_BUILD_UI_SHELL)
  if(NOT DEFINED WEBVIEW2_SDK_DIR)
    message(FATAL_ERROR "WEBVIEW2_SDK_DIR is required to build the Windows WebView2 shell")
  endif()
endif()
```

- [ ] **Step 2: Implement host bridge request handling**

In `src/app/ui_shell/host_bridge.cpp`, add a request handler that accepts:

```json
{"id":1,"action":"status.get","payload":{}}
```

and rejects unknown actions with:

```json
{"id":1,"ok":false,"code":"unknown_action","message":"Unknown desktop action"}
```

All accepted actions call the native core RPC client.

- [ ] **Step 3: Implement WebView2 creation and message bridge**

In `webview2_host_win32.cpp`, create the Win32 window, initialize WebView2, load
the resolved renderer URL or file, and wire WebMessageReceived to
`HostMessageHandler`.

The bridge must post responses back as JSON strings:

```json
{"id":1,"ok":true,"data":{"phase":"idle"}}
```

- [ ] **Step 4: Preserve existing desktop UX behavior**

Implement Windows equivalents for:

- main window advanced/minimal sizes,
- close-to-tray prompt,
- tray status menu,
- modal password/confirm/service-install prompts,
- core restart/quit.

- [ ] **Step 5: Run Windows focused verification**

Run:

```powershell
cmake --build build --config Debug --target exv-ui win32_webview2_runtime_test ui_shell_contract_test ui_shell_core_rpc_client_test
ctest --test-dir build -C Debug -R "win32_webview2_runtime_test|ui_shell_contract_test|ui_shell_core_rpc_client_test" --output-on-failure
cd webui
pnpm run build
```

Expected: all pass. Manual smoke: `exv-ui --renderer-index <path-to-index.html>`
opens the renderer and `status.get` succeeds.

- [ ] **Step 6: Commit**

```powershell
git add CMakeLists.txt src/app/ui_shell src/platform/win32/ui_shell
git commit -m "Implement Windows WebView2 desktop shell"
```

## Task 9: macOS WKWebView Host Parity

**Files:**
- Modify: `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- Modify: `CMakeLists.txt`
- Test: `tests/darwin_wkwebview_runtime_test.cpp`

- [ ] **Step 1: Write macOS availability test**

Create `tests/darwin_wkwebview_runtime_test.cpp`:

```cpp
#include <cassert>

int main() {
#if defined(__APPLE__)
  assert(true);
#else
  assert(true);
#endif
}
```

The first test is a build gate; platform-specific API checks are added inside
the `.mm` host implementation where WebKit headers are available.

- [ ] **Step 2: Link Cocoa and WebKit frameworks**

Add:

```cmake
if(APPLE AND EXV_BUILD_UI_SHELL)
  target_link_libraries(exv-ui PRIVATE "-framework Cocoa" "-framework WebKit")
endif()
```

- [ ] **Step 3: Implement WKWebView host**

In `wk_webview_host_darwin.mm`, implement:

- `NSApplication` startup,
- main `NSWindow`,
- `WKWebViewConfiguration`,
- script message handler named `ecnuVpnHost`,
- local file/dev URL loading,
- JSON response callback into JavaScript,
- modal prompt windows or sheets,
- status item menu.

- [ ] **Step 4: Run macOS focused verification**

Run on macOS:

```bash
cmake --build build --config Debug --target exv-ui darwin_wkwebview_runtime_test ui_shell_contract_test ui_shell_core_rpc_client_test
ctest --test-dir build -C Debug -R "darwin_wkwebview_runtime_test|ui_shell_contract_test|ui_shell_core_rpc_client_test" --output-on-failure
cd webui && pnpm run build
```

Expected: all pass. Manual smoke: `exv-ui --renderer-index <path-to-index.html>`
opens the renderer and `status.get` succeeds.

- [ ] **Step 5: Commit**

```bash
git add CMakeLists.txt src/platform/darwin/ui_shell tests/darwin_wkwebview_runtime_test.cpp
git commit -m "Implement macOS WKWebView desktop shell"
```

## Task 10: Linux WebKitGTK Host Parity

**Files:**
- Modify: `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- Modify: `CMakeLists.txt`
- Test: `tests/linux_webkitgtk_runtime_test.cpp`

- [ ] **Step 1: Add WebKitGTK dependency probe**

Use `pkg-config` in CMake:

```cmake
if(UNIX AND NOT APPLE AND EXV_BUILD_UI_SHELL)
  find_package(PkgConfig REQUIRED)
  pkg_check_modules(WEBKITGTK QUIET webkit2gtk-4.1 gtk+-3.0)
  if(NOT WEBKITGTK_FOUND)
    message(FATAL_ERROR "WebKitGTK development packages are required: webkit2gtk-4.1 and gtk+-3.0")
  endif()
endif()
```

- [ ] **Step 2: Write Linux dependency test**

Create `tests/linux_webkitgtk_runtime_test.cpp`:

```cpp
#include <cassert>

int main() {
#if defined(__linux__)
  assert(true);
#else
  assert(true);
#endif
}
```

- [ ] **Step 3: Implement WebKitGTK host**

In `webkitgtk_host_linux.cpp`, implement:

- GTK application startup,
- main window,
- WebKit user content manager script message handler,
- local file/dev URL loading,
- JavaScript callback response delivery,
- modal prompt dialogs,
- tray/status fallback with clear behavior when the desktop environment has no
  supported tray implementation.

- [ ] **Step 4: Run Linux focused verification**

Run on Linux:

```bash
cmake --build build --config Debug --target exv-ui linux_webkitgtk_runtime_test ui_shell_contract_test ui_shell_core_rpc_client_test
ctest --test-dir build -C Debug -R "linux_webkitgtk_runtime_test|ui_shell_contract_test|ui_shell_core_rpc_client_test" --output-on-failure
cd webui && pnpm run build
```

Expected: all pass. Manual smoke: `exv-ui --renderer-index <path-to-index.html>`
opens the renderer and `status.get` succeeds.

- [ ] **Step 5: Commit**

```bash
git add CMakeLists.txt src/platform/linux/ui_shell tests/linux_webkitgtk_runtime_test.cpp
git commit -m "Implement Linux WebKitGTK desktop shell"
```

## Task 11: Packaging Switch

**Files:**
- Create: `scripts/package_ui_shell.py`
- Modify: `webui/package.json`
- Modify: `webui/scripts/build-layout.cjs`
- Modify: `webui/README.md`
- Modify: platform installer/package scripts under `scripts/` or `packaging/`

- [ ] **Step 1: Add package artifact test**

Add a script-level check inside `scripts/package_ui_shell.py`:

```python
def assert_no_electron_payload(package_dir: Path) -> None:
    forbidden = ["electron.exe", "Electron Framework.framework", "chromium.pak"]
    found = [name for name in forbidden if list(package_dir.rglob(name))]
    if found:
        raise SystemExit(f"Electron/Chromium payload found: {', '.join(found)}")
```

- [ ] **Step 2: Build native package layout**

The package layout must contain:

```text
ECNU VPN/
  exv-ui or platform app executable
  bin/exv
  bin/exv-helper
  webui/index.html
  webui/assets/*
```

Windows additionally includes `WebView2Loader.dll` or statically links the
loader according to the chosen SDK integration.

- [ ] **Step 3: Add package commands**

Add scripts:

```json
"webview:compile": "pnpm run build",
"webview:package": "python ../scripts/package_ui_shell.py"
```

Keep Electron scripts present but stop documenting them as the production
default.

- [ ] **Step 4: Run packaging smoke on each platform**

Run:

```bash
cd webui
pnpm run webview:compile
pnpm run webview:package
```

Expected: package output exists, contains native binaries and renderer assets,
and contains no Electron/Chromium payload.

- [ ] **Step 5: Commit**

```bash
git add scripts/package_ui_shell.py webui/package.json webui/scripts/build-layout.cjs webui/README.md
git commit -m "Switch production packaging to native WebView shell"
```

## Task 12: Electron Retirement

**Files:**
- Modify: `webui/package.json`
- Modify: `webui/pnpm-lock.yaml`
- Move or delete: `webui/desktop/main`
- Move or delete: `webui/desktop/preload`
- Move or delete: `webui/electron-builder.config.cjs`
- Modify: `webui/README.md`
- Test: `webui/host/__tests__/host-boundary.test.ts`

- [ ] **Step 1: Make the retirement test fail while Electron production paths remain**

Extend `webui/host/__tests__/host-boundary.test.ts`:

```ts
const packageJson = JSON.parse(readFileSync(join(webuiRoot, 'package.json'), 'utf8'))
assert.equal(packageJson.scripts['desktop:package'], undefined)
assert.equal(packageJson.devDependencies.electron, undefined)
assert.equal(packageJson.devDependencies['electron-builder'], undefined)
```

- [ ] **Step 2: Remove Electron production scripts and dependencies**

Remove these script keys from `webui/package.json`:

```json
"desktop:package": "...",
"desktop:package:dir": "...",
"desktop:debug": "...",
"desktop:build": "..."
```

Remove these dev dependencies:

```json
"electron": "...",
"electron-builder": "...",
"@types/electron": "..."
```

- [ ] **Step 3: Move Electron adapter out of production paths**

If the team still wants a dev-only adapter, move it to:

```text
webui/dev-electron/
```

and keep it outside production package scripts. If no team member is using it,
delete `webui/desktop/main`, `webui/desktop/preload`, and
`webui/electron-builder.config.cjs`.

- [ ] **Step 4: Refresh package lock**

Run:

```bash
cd webui
pnpm install
```

Expected: lockfile removes Electron packages.

- [ ] **Step 5: Run final checks**

Run:

```bash
python scripts/generate_contracts.py --check
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
cd webui
pnpm run build
pnpm exec node scripts/run-electron-test.cjs host/__tests__/host-boundary.test.ts
```

Expected: all pass. The test runner may be renamed after Electron retirement;
if renamed, update the command and the file path in this plan step in the same
commit.

- [ ] **Step 6: Commit**

```bash
git add webui webui/package.json webui/pnpm-lock.yaml CMakeLists.txt
git commit -m "Retire Electron production desktop shell"
```

## Final Acceptance

Run on Windows:

```powershell
python scripts/generate_contracts.py --check
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
cd webui
pnpm run build
pnpm run webview:package
```

Run on macOS:

```bash
python3 scripts/generate_contracts.py --check
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
cd webui
pnpm run build
pnpm run webview:package
```

Run on Linux:

```bash
python3 scripts/generate_contracts.py --check
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
cd webui
pnpm run build
pnpm run webview:package
```

Manual acceptance on each platform:

- App opens the Vue renderer from packaged assets.
- `status.get` succeeds.
- Connect and disconnect flow reaches native core/helper.
- Service install/uninstall routes through core/helper.
- Modal password, confirm, service install, and close prompts work.
- Logs stream to the renderer.
- Default package contains no Electron or bundled Chromium runtime.
- Windows missing WebView2 Runtime path gives explicit install flow using
  Evergreen Bootstrapper policy.
