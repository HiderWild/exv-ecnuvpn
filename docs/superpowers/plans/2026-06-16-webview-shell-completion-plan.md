# Cross-Platform WebView Shell Completion Implementation Plan

> Execution status on 2026-06-16: Phase 1 through Phase 8 have been
> implemented on branch `codex/ui-framework-webview-shell`. Windows, macOS,
> and Linux acceptance evidence is recorded in
> `docs/superpowers/reports/2026-06-16-webview-shell-acceptance-report.md`.
> macOS acceptance was run through SSH host `macmini` using a clean temporary
> worktree because the synced macOS workspace contains unrelated dirty state.
> Linux acceptance was run in WSL `Ubuntu-24.04` with Node 22, CMake 3.28,
> Clang 18, and WebKitGTK 4.1 development packages.
> macOS/Linux core process transport no longer uses the closed fallback:
> `src/app/ui_shell/core_process_manager.cpp` now has a POSIX
> posix_spawn/stdin/stdout transport. Focused macOS verification passed on
> `macmini` in a clean temporary worktree with Homebrew LLVM.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the remaining WebView shell migration so Windows, macOS, and Linux production desktop packages use real platform WebView hosts, Electron is retired from production packaging, and tests prove the final state.

**Architecture:** Keep the Vue renderer and generated desktop RPC contract as the UI boundary. The C++ `exv-ui` process owns the native shell lifecycle, launches or connects to `exv` through a real core RPC transport, and delegates only window/WebView details to `src/platform/<os>/ui_shell`. Electron is retired from production packaging; remaining work is platform-host parity and host-specific acceptance.

**Tech Stack:** Vue 3, TypeScript, Vite, C++20, CMake 3.28+, WebView2 Evergreen Runtime, Cocoa/WKWebView, GTK/WebKitGTK, existing generated contract pipeline.

---

## Current Audit Result

Audit date: 2026-06-16.

Implemented and committed evidence:

- `src/app/ui_shell/ui_shell_main.cpp` constructs a core process transport, constructs a platform `UiWindow`, and calls `run_ui_shell_window(...)`.
- Windows has a real WebView2 host path under `EXV_BUILD_UI_SHELL`, WebView2 runtime detection, and controlled Evergreen bootstrapper policy.
- Native WebView package scripts now produce `build/<platform>/webview/package/ECNU VPN` layouts.
- Electron production dependencies, scripts, adapter sources, and package docs have been removed from active production paths.
- Windows local acceptance, macOS acceptance on `macmini`, and Linux acceptance
  in WSL `Ubuntu-24.04` passed and are
  recorded in
  `docs/superpowers/reports/2026-06-16-webview-shell-acceptance-report.md`.

Reusable acceptance entrypoints:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\accept-webview-shell-windows.ps1
ssh macmini "cd /Users/tomli/Development/Projects/CPP/ECNU-VPN && bash scripts/accept-webview-shell-macos.sh"
bash scripts/accept-webview-shell-linux.sh
```

These scripts write logs under `build/webview-acceptance/<platform>/`.

## Completion Status Against Original Plan

| Original task | Status | Evidence | Remaining gap |
| --- | --- | --- | --- |
| Task 1 Neutral Host Contract | Done | `webui/host`, generated desktop contract tests, `host-boundary.test.ts`. | None for contract extraction. |
| Task 2 Renderer API Neutrality | Done | Renderer/host tests block Electron and Node desktop imports from renderer paths. | None currently known. |
| Task 3 Common Native UI Shell Skeleton | Done | `src/app/ui_shell/*`, `exv-ui`, `run_ui_shell_window(...)`, sidecar args. | None currently known. |
| Task 4 Native Core RPC Client For UI Shell | Done | `CoreRpcClient`, Windows pipe transport, POSIX posix_spawn/stdin/stdout transport, production startup wiring tests, Linux acceptance. | None currently known. |
| Task 5 Windows WebView2 Runtime Detection | Done | `src/platform/win32/ui_shell/webview2_runtime_win32.cpp`; focused tests pass. | None currently known. |
| Task 6 Windows WebView2 Bootstrapper Policy | Done | Allowlist, runner seam, and host startup policy are present. | Manual UX validation still belongs to Windows acceptance. |
| Task 7 Platform WebView Host Interfaces | Done | All platforms expose `UiWindow` factories. | None currently known. |
| Task 8 Windows WebView2 Host Parity | Done with failure-path caveat | Windows host creates native window/WebView2 bridge under `EXV_BUILD_UI_SHELL`; Windows acceptance passed. | Failure paths still return `70` when runtime/bootstrap/COM/window creation cannot proceed. |
| Task 9 macOS WKWebView Host Parity | Done | `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm` creates Cocoa/WKWebView, registers script message handling, dispatches host RPC, and macOS acceptance passed on `macmini`. Focused macOS rebuild also covered POSIX core transport plus `exv-ui`. | None currently known. |
| Task 10 Linux WebKitGTK Host Parity | Done | `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp` creates GTK/WebKitGTK window, registers script message handling, dispatches host RPC, posts renderer events under `EXV_BUILD_UI_SHELL`, and Linux acceptance passed. | None currently known. |
| Task 11 Packaging Switch | Done | Build scripts, smoke scripts, start script, docs, package verifier use WebView layout; Linux package output passed. | None currently known. |
| Task 12 Electron Retirement | Done | Electron production package dependencies/scripts/source paths removed. | None currently known. |

## Quality Gaps To Fix

1. No remaining platform acceptance gap is currently known.

2. Lower phases in this document are the historical implementation recipe. The
   authoritative completion evidence is the table above and the acceptance
   report.

## Target Boundaries

- `webui/src`: renderer only. It imports the neutral host client and must not import Electron, WebView2, WKWebView, WebKitGTK, or Node desktop APIs.
- `webui/host`: TypeScript neutral host contract and client helpers only.
- `src/app/ui_shell`: cross-platform shell orchestration, renderer asset resolution, core process lifecycle, core RPC transport, host bridge, and `UiWindow` abstraction.
- `src/platform/win32/ui_shell`: Win32/WebView2 window, runtime detection, Evergreen bootstrap execution, and Windows-specific UI integration.
- `src/platform/darwin/ui_shell`: Cocoa/WKWebView window, script message handler, and macOS-specific UI integration.
- `src/platform/linux/ui_shell`: GTK/WebKitGTK window, user content manager bridge, and Linux-specific UI integration.
- `scripts/package_ui_shell.py`: native WebView package layout only; it must never package Electron or Chromium.
- Electron adapter files must not exist in active production paths.

---

## Phase 1: Make Production `exv-ui` Use The Neutral Runtime

**Files:**
- Modify: `src/app/ui_shell/core_process_manager.hpp`
- Modify: `src/app/ui_shell/core_process_manager.cpp`
- Modify: `src/app/ui_shell/ui_shell_main.cpp`
- Modify: `tests/ui_shell_core_rpc_client_test.cpp`
- Create: `tests/ui_shell_main_wiring_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Add a failing wiring test for production startup shape**

Create `tests/ui_shell_main_wiring_test.cpp`:

```cpp
#include <fstream>
#include <iostream>
#include <string>

#ifndef ECNUVPN_SOURCE_DIR
#error "ECNUVPN_SOURCE_DIR must be defined"
#endif

namespace {
std::string read_file(const std::string &path) {
  std::ifstream input(path);
  return {std::istreambuf_iterator<char>(input),
          std::istreambuf_iterator<char>()};
}

bool contains(const std::string &text, const std::string &needle) {
  return text.find(needle) != std::string::npos;
}
} // namespace

int main() {
  const std::string root = ECNUVPN_SOURCE_DIR;
  const std::string main_cpp = read_file(root + "/src/app/ui_shell/ui_shell_main.cpp");
  int failures = 0;
  if (!contains(main_cpp, "run_ui_shell_window(")) {
    std::cerr << "exv-ui main must run the neutral runtime\n";
    ++failures;
  }
  if (!contains(main_cpp, "create_core_process_transport(")) {
    std::cerr << "exv-ui main must create a production core RPC transport\n";
    ++failures;
  }
  if (contains(main_cpp, "return ecnuvpn::platform::win32::ui_shell::run_webview2_host(config);")) {
    std::cerr << "exv-ui main must not bypass run_ui_shell_window on Windows\n";
    ++failures;
  }
  return failures == 0 ? 0 : 1;
}
```

- [ ] **Step 2: Register and run the failing test**

Modify `CMakeLists.txt` near the other UI shell tests:

```cmake
add_executable(ui_shell_main_wiring_test
    tests/ui_shell_main_wiring_test.cpp
)
target_compile_definitions(ui_shell_main_wiring_test PRIVATE
    ECNUVPN_SOURCE_DIR="${CMAKE_SOURCE_DIR}"
)
add_test(NAME ui_shell_main_wiring_test COMMAND ui_shell_main_wiring_test)
```

Run:

```powershell
cmake --build build --config Debug --target ui_shell_main_wiring_test
ctest --test-dir build -C Debug -R ui_shell_main_wiring_test --output-on-failure
```

Expected before implementation: FAIL because `ui_shell_main.cpp` bypasses the neutral runtime.

- [ ] **Step 3: Define the production core transport factory**

Modify `src/app/ui_shell/core_process_manager.hpp`:

```cpp
#pragma once

#include "app/ui_shell/core_rpc_client.hpp"

#include <memory>
#include <string>
#include <vector>

namespace ecnuvpn::ui_shell {

struct CoreProcessLaunch {
  std::string exv_path;
  std::string state_dir;
  std::string runtime_dir;
  bool use_stdin = true;
};

class CoreProcessManager {
public:
  virtual ~CoreProcessManager() = default;
  virtual bool start(const CoreProcessLaunch &launch) = 0;
  virtual void stop() = 0;
  virtual bool alive() const = 0;
};

std::vector<std::string> build_core_process_arguments(
    const CoreProcessLaunch &launch);

std::unique_ptr<CoreRpcTransport> create_core_process_transport(
    const CoreProcessLaunch &launch);

} // namespace ecnuvpn::ui_shell
```

- [ ] **Step 4: Implement a Windows production transport and a clear non-Windows failure**

Modify `src/app/ui_shell/core_process_manager.cpp`. Keep `build_core_process_arguments(...)` unchanged and add this implementation at the end of the namespace:

```cpp
#include <memory>

#if defined(_WIN32)
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#endif

namespace ecnuvpn::ui_shell {

namespace {

#if defined(_WIN32)
class WindowsCoreProcessTransport final : public CoreRpcTransport {
public:
  explicit WindowsCoreProcessTransport(const CoreProcessLaunch &launch) {
    // Implementation owns CreateProcessW, stdin write pipe, stdout read pipe,
    // and process termination. Keep this class private to the shell layer.
  }

  ~WindowsCoreProcessTransport() override {
    // Close handles, then terminate the child only if it is still alive.
  }

  bool write_line(const std::string &line) override {
    // Write UTF-8 line plus '\n' to child stdin.
    return false;
  }

  bool read_line(std::string &line) override {
    // Block until a complete line is available on child stdout.
    return false;
  }

  bool read_available_line(std::string &line) override {
    // Non-blocking poll used by UiWindow::pump_core_events.
    return false;
  }
};
#endif

class UnsupportedCoreProcessTransport final : public CoreRpcTransport {
public:
  bool write_line(const std::string &) override { return false; }
  bool read_line(std::string &) override { return false; }
};

} // namespace

std::unique_ptr<CoreRpcTransport> create_core_process_transport(
    const CoreProcessLaunch &launch) {
#if defined(_WIN32)
  return std::make_unique<WindowsCoreProcessTransport>(launch);
#else
  (void)launch;
  return std::make_unique<UnsupportedCoreProcessTransport>();
#endif
}

} // namespace ecnuvpn::ui_shell
```

Then replace the method bodies with real pipe/process code before marking this step complete. Required behavior:

- `write_line` appends exactly one newline.
- `read_line` strips trailing `\r`.
- `read_available_line` returns immediately when no full line is available.
- destructor closes stdin first, drains process shutdown briefly, then terminates only if still running.
- launch uses `build_core_process_arguments(launch)` and `launch.exv_path`.

- [ ] **Step 5: Wire `ui_shell_main.cpp` through `run_ui_shell_window`**

Modify `src/app/ui_shell/ui_shell_main.cpp` so each platform creates a concrete `UiWindow`, then uses one cross-platform runtime path:

```cpp
#include "app/ui_shell/core_process_manager.hpp"
#include "app/ui_shell/core_rpc_client.hpp"
#include "app/ui_shell/ui_shell_runtime.hpp"
```

The bottom of `main(...)` should follow this shape:

```cpp
auto transport = ecnuvpn::ui_shell::create_core_process_transport(
    ecnuvpn::ui_shell::CoreProcessLaunch{options.exv_path, "", "", true});
ecnuvpn::ui_shell::CoreRpcClient client(*transport);

#if defined(_WIN32)
auto window = ecnuvpn::platform::win32::ui_shell::create_webview2_window();
#elif defined(__APPLE__)
auto window = ecnuvpn::platform::darwin::ui_shell::create_wk_webview_window();
#elif defined(__linux__)
auto window = ecnuvpn::platform::linux_ui_shell::create_webkitgtk_window();
#else
std::cerr << "exv-ui: unsupported platform\n";
return 70;
#endif

return ecnuvpn::ui_shell::run_ui_shell_window(*window, config, client);
```

- [ ] **Step 6: Run focused verification and commit**

Run:

```powershell
cmake --build build --config Debug --target ui_shell_main_wiring_test ui_shell_core_rpc_client_test ui_shell_runtime_test
ctest --test-dir build -C Debug -R "ui_shell_main_wiring_test|ui_shell_core_rpc_client_test|ui_shell_runtime_test" --output-on-failure
git add src/app/ui_shell/core_process_manager.hpp src/app/ui_shell/core_process_manager.cpp src/app/ui_shell/ui_shell_main.cpp tests/ui_shell_main_wiring_test.cpp CMakeLists.txt
git commit -m "Wire native UI shell through neutral runtime"
```

Expected after implementation: all listed tests pass.

## Phase 2: Replace Platform Host Stubs With `UiWindow` Implementations

**Files:**
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.hpp`
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Modify: `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- Modify: `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- Modify: `tests/win32_webview2_runtime_test.cpp`
- Modify: `tests/darwin_wkwebview_runtime_test.cpp`
- Modify: `tests/linux_webkitgtk_runtime_test.cpp`

- [ ] **Step 1: Change platform APIs from run-functions to window factories**

Windows header target shape:

```cpp
#pragma once

#include "app/ui_shell/ui_window.hpp"

#include <memory>

namespace ecnuvpn::platform::win32::ui_shell {

std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_webview2_window();

} // namespace ecnuvpn::platform::win32::ui_shell
```

Darwin and Linux should expose the same pattern from their `.mm` / `.cpp` translation units:

```cpp
std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_wk_webview_window();
std::unique_ptr<ecnuvpn::ui_shell::UiWindow> create_webkitgtk_window();
```

- [ ] **Step 2: Update tests so stubs fail**

Replace the stub-return assertions in `tests/darwin_wkwebview_runtime_test.cpp` and `tests/linux_webkitgtk_runtime_test.cpp` with factory assertions:

```cpp
auto window = ecnuvpn::platform::darwin::ui_shell::create_wk_webview_window();
return window ? 0 : 1;
```

```cpp
auto window = ecnuvpn::platform::linux_ui_shell::create_webkitgtk_window();
return window ? 0 : 1;
```

For Windows, assert the factory exists and that `dispatch_webview2_host_message(...)` remains covered:

```cpp
auto window = create_webview2_window();
assert(window != nullptr);
```

Run:

```powershell
cmake --build build --config Debug --target win32_webview2_runtime_test
ctest --test-dir build -C Debug -R win32_webview2_runtime_test --output-on-failure
```

Expected before factory implementation: FAIL to compile.

- [ ] **Step 3: Implement minimal non-stub `UiWindow` classes**

Each platform file must define a class derived from `ecnuvpn::ui_shell::UiWindow` with these behaviors:

```cpp
class PlatformUiWindow final : public ecnuvpn::ui_shell::UiWindow {
public:
  void set_message_handler(ecnuvpn::ui_shell::HostMessageHandler handler) override {
    handler_ = std::move(handler);
  }

  int run(const ecnuvpn::ui_shell::UiWindowConfig &config) override {
    // Create native app/window/WebView, load config.renderer, and enter loop.
    return 0;
  }

  void emit_event(const std::string &event_json) override {
    // Post event JSON to renderer without blocking the native UI thread.
  }

private:
  ecnuvpn::ui_shell::HostMessageHandler handler_;
};
```

The final implementation must not return `70` from `run(...)` for supported platforms when SDK dependencies are enabled.

- [ ] **Step 4: Run focused platform compile gates and commit**

Windows:

```powershell
cmake --build build --config Debug --target win32_webview2_runtime_test exv-ui
ctest --test-dir build -C Debug -R win32_webview2_runtime_test --output-on-failure
```

macOS:

```bash
cmake --build build --config Debug --target darwin_wkwebview_runtime_test exv-ui
ctest --test-dir build -C Debug -R darwin_wkwebview_runtime_test --output-on-failure
```

Linux:

```bash
cmake --build build --config Debug --target linux_webkitgtk_runtime_test exv-ui
ctest --test-dir build -C Debug -R linux_webkitgtk_runtime_test --output-on-failure
```

Commit:

```bash
git add src/platform/win32/ui_shell src/platform/darwin/ui_shell src/platform/linux/ui_shell tests CMakeLists.txt
git commit -m "Introduce native WebView window factories"
```

## Phase 3: Implement Windows WebView2 Runtime And Bridge

**Files:**
- Modify: `src/platform/win32/ui_shell/webview2_runtime_win32.hpp`
- Modify: `src/platform/win32/ui_shell/webview2_runtime_win32.cpp`
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Modify: `tests/win32_webview2_runtime_test.cpp`

- [ ] **Step 1: Add failing tests for bootstrapper execution seam**

Extend `tests/win32_webview2_runtime_test.cpp` with:

```cpp
bool invoked = false;
const bool ran = run_webview2_evergreen_bootstrapper_with_runner(
    "https://go.microsoft.com/fwlink/?linkid=2124703",
    "C:/temp/MicrosoftEdgeWebview2Setup.exe",
    [&](const std::string &installer, const std::string &args) {
      invoked = true;
      assert(installer == "C:/temp/MicrosoftEdgeWebview2Setup.exe");
      assert(args == "/silent /install");
      return true;
    });
assert(ran);
assert(invoked);
```

Expected before implementation: FAIL because the seam does not exist.

- [ ] **Step 2: Add controlled bootstrapper runner**

Add to `webview2_runtime_win32.hpp`:

```cpp
using WebView2BootstrapRunner =
    std::function<bool(const std::string &installer_path,
                       const std::string &installer_args)>;

bool run_webview2_evergreen_bootstrapper_with_runner(
    const std::string &download_url,
    const std::string &installer_path,
    const WebView2BootstrapRunner &runner);
```

Implement in `.cpp`:

```cpp
bool run_webview2_evergreen_bootstrapper_with_runner(
    const std::string &download_url,
    const std::string &installer_path,
    const WebView2BootstrapRunner &runner) {
  if (!is_allowed_webview2_bootstrapper_url(download_url) ||
      installer_path.empty() || !runner) {
    return false;
  }
  return runner(installer_path, "/silent /install");
}
```

Then implement `run_webview2_evergreen_bootstrapper(...)` by downloading only the allowlisted Microsoft URL to a temp file and calling `CreateProcessW` with `/silent /install`. If download fails, return `false` and surface an explicit UI error from the host.

- [ ] **Step 3: Implement WebView2 window creation and script bridge**

`webview2_host_win32.cpp` must:

- call `detect_webview2_runtime()` before WebView creation;
- show a native prompt before online bootstrapper execution;
- create a Win32 top-level window;
- call `CreateCoreWebView2EnvironmentWithOptions`;
- call `CreateCoreWebView2Controller`;
- navigate to `config.renderer.location` as either dev URL or `file:///.../index.html`;
- register `add_WebMessageReceived`;
- call `handler_(message_json)` for renderer requests;
- post the response back with `PostWebMessageAsJson`;
- execute `emit_event(...)` by posting a host event JSON envelope to the renderer.

The bridge payload from renderer to native must be:

```json
{"id":1,"action":"status.get","payload":{}}
```

The native response posted back must remain:

```json
{"id":1,"ok":true,"data":{}}
```

- [ ] **Step 4: Run Windows verification and commit**

```powershell
cmake --preset windows-release -DEXV_BUILD_UI_SHELL=ON -DWEBVIEW2_SDK_DIR=$env:WEBVIEW2_SDK_DIR
cmake --build build/windows/cpp --config Release --target exv-ui win32_webview2_runtime_test
ctest --test-dir build/windows/cpp -C Release -R "win32_webview2_runtime_test|ui_shell_.*" --output-on-failure
git add src/platform/win32/ui_shell tests/win32_webview2_runtime_test.cpp
git commit -m "Implement Windows WebView2 UI shell"
```

Expected: Release configure succeeds with SDK present; tests pass; `exv-ui.exe --exv <path> --renderer-url http://127.0.0.1:8288` opens a WebView2 window.

## Phase 4: Implement macOS WKWebView Host

**Files:**
- Modify: `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- Modify: `tests/darwin_wkwebview_runtime_test.cpp`
- Modify: `scripts/build-macos.sh`
- Modify: `scripts/macos-packaging-smoke.sh`

- [ ] **Step 1: Add compile checks for Cocoa/WKWebView symbols**

Extend `tests/darwin_wkwebview_runtime_test.cpp`:

```cpp
auto window = ecnuvpn::platform::darwin::ui_shell::create_wk_webview_window();
if (!window) {
  return 1;
}
ecnuvpn::ui_shell::UiWindowConfig config;
config.renderer = ecnuvpn::ui_shell::resolve_renderer_assets("http://127.0.0.1:8288", "");
config.exv_path = "/tmp/exv";
window->set_message_handler([](const std::string &) {
  return std::string(R"({"id":1,"ok":true,"data":{}})");
});
window->emit_event(R"({"type":"status","data":{}})");
return 0;
```

Expected before implementation: compile or link failure if the factory is not real.

- [ ] **Step 2: Implement Cocoa application and WKWebView bridge**

`wk_webview_host_darwin.mm` must create:

- `NSApplication`;
- `NSWindow`;
- `WKWebViewConfiguration`;
- `WKUserContentController`;
- `WKScriptMessageHandler` named `ecnuVpnHost`;
- a JavaScript shim that defines `window.ecnuVpn.invoke(request)`;
- a response callback map keyed by `id`;
- `emit_event(...)` dispatch to the main queue.

The message handler must pass the JSON string to `HostMessageHandler` and return the JSON response to the renderer callback.

- [ ] **Step 3: Run macOS verification and commit**

```bash
cmake --preset macos-release -DEXV_BUILD_UI_SHELL=ON
cmake --build build/macos/cpp --config Release --target exv-ui darwin_wkwebview_runtime_test
ctest --test-dir build/macos/cpp -C Release -R "darwin_wkwebview_runtime_test|ui_shell_.*" --output-on-failure
scripts/build-macos.sh desktop
scripts/macos-packaging-smoke.sh
git add src/platform/darwin/ui_shell tests/darwin_wkwebview_runtime_test.cpp scripts/build-macos.sh scripts/macos-packaging-smoke.sh
git commit -m "Implement macOS WKWebView UI shell"
```

Expected: native macOS package contains `exv-ui`, `bin/exv`, `bin/exv-helper`, `webui/index.html`, and no Electron app bundle.

## Phase 5: Implement Linux WebKitGTK Host

**Files:**
- Modify: `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- Modify: `tests/linux_webkitgtk_runtime_test.cpp`
- Modify: `scripts/build-linux.sh`

- [ ] **Step 1: Add compile checks for GTK/WebKitGTK symbols**

Extend `tests/linux_webkitgtk_runtime_test.cpp`:

```cpp
auto window = ecnuvpn::platform::linux_ui_shell::create_webkitgtk_window();
if (!window) {
  return 1;
}
window->set_message_handler([](const std::string &) {
  return std::string(R"({"id":1,"ok":true,"data":{}})");
});
window->emit_event(R"({"type":"status","data":{}})");
return 0;
```

- [ ] **Step 2: Implement GTK/WebKitGTK bridge**

`webkitgtk_host_linux.cpp` must:

- initialize GTK;
- create `GtkApplicationWindow`;
- create `WebKitUserContentManager`;
- register a script message handler named `ecnuVpnHost`;
- create `WebKitWebView`;
- load dev URL or packaged `file://` URI;
- call `HostMessageHandler` from the UI thread;
- post JSON responses and event envelopes back to renderer JavaScript.

- [ ] **Step 3: Run Linux verification and commit**

```bash
cmake --preset linux-release -DEXV_BUILD_UI_SHELL=ON
cmake --build build/linux/cpp --config Release --target exv-ui linux_webkitgtk_runtime_test
ctest --test-dir build/linux/cpp -C Release -R "linux_webkitgtk_runtime_test|ui_shell_.*" --output-on-failure
scripts/build-linux.sh desktop
git add src/platform/linux/ui_shell tests/linux_webkitgtk_runtime_test.cpp scripts/build-linux.sh
git commit -m "Implement Linux WebKitGTK UI shell"
```

Expected when dependencies are present: configure, build, and focused tests pass. Expected when dependencies are missing: CMake fails with the existing actionable WebKitGTK package message.

## Phase 6: Package Smoke And Documentation Switch

**Files:**
- Modify: `scripts/package_ui_shell.py`
- Modify: `scripts/windows-packaging-smoke.ps1`
- Modify: `scripts/macos-packaging-smoke.sh`
- Modify: `scripts/validate-merge-prep-windows.ps1`
- Modify: `scripts/validate-merge-prep-macos.sh`
- Modify: `start.ps1`
- Modify: `README.md`
- Modify: `docs/build_guide.md`
- Modify: `docs/user_guide.md`
- Move: `docs/windows-electron-helper-recovery.md` to `docs/superpowers/archive/windows-electron-helper-recovery.md`

- [ ] **Step 1: Add package launch smoke checks**

`scripts/package_ui_shell.py` already writes `exv-ui.args`. Add a `--verify-launch-targets-only` mode that validates an existing package without copying files:

```python
parser.add_argument("--verify-launch-targets-only", action="store_true")
parser.add_argument("--package-dir", type=Path)
```

When enabled:

```python
if args.verify_launch_targets_only:
    if not args.package_dir:
        raise SystemExit("--package-dir is required with --verify-launch-targets-only")
    validate_launch_args_targets(args.package_dir)
    assert_no_electron_payload(args.package_dir)
    print(f"verified native WebView shell package: {args.package_dir}")
    return 0
```

- [ ] **Step 2: Update smoke scripts to WebView package layout**

`scripts/windows-packaging-smoke.ps1` must search:

```powershell
$PackageRoot = Join-Path $RepoRoot 'build\windows\webview\package\ECNU VPN'
$UiShell = Join-Path $PackageRoot 'exv-ui.exe'
$Core = Join-Path $PackageRoot 'bin\exv.exe'
$Helper = Join-Path $PackageRoot 'bin\exv-helper.exe'
```

It must reject:

```powershell
Get-ChildItem -Path $PackageRoot -Recurse -Include electron.exe,chromium.pak
```

`scripts/macos-packaging-smoke.sh` must search:

```bash
PACKAGE_ROOT="$REPO_ROOT/build/macos/webview/package/ECNU VPN"
UI_SHELL="$PACKAGE_ROOT/exv-ui"
CORE="$PACKAGE_ROOT/bin/exv"
HELPER="$PACKAGE_ROOT/bin/exv-helper"
```

It must reject:

```bash
find "$PACKAGE_ROOT" \( -name 'Electron Framework.framework' -o -name 'chromium.pak' \) -print -quit
```

- [ ] **Step 3: Remove Electron as the documented production path**

Update `README.md`, `docs/build_guide.md`, and `docs/user_guide.md` so:

- WebView shell is the default desktop package;
- Electron is described only as a temporary migration adapter while it exists;
- build outputs point to `build/<platform>/webview/package/ECNU VPN`;
- `start.ps1 desktop` launches or builds WebView package paths, not Electron paths.

Move the Electron recovery document:

```powershell
Move-Item -LiteralPath docs/windows-electron-helper-recovery.md -Destination docs/superpowers/archive/windows-electron-helper-recovery.md
```

- [ ] **Step 4: Run docs and package policy verification and commit**

```powershell
cmake --build build --config Debug --target native_packaging_policy_test ui_shell_cmake_policy_test
ctest --test-dir build -C Debug -R "native_packaging_policy_test|ui_shell_cmake_policy_test" --output-on-failure
cd webui
pnpm exec node scripts/run-electron-test.cjs host/__tests__/webview-package-policy.test.ts
cd ..
git add scripts/package_ui_shell.py scripts/windows-packaging-smoke.ps1 scripts/macos-packaging-smoke.sh scripts/validate-merge-prep-windows.ps1 scripts/validate-merge-prep-macos.sh start.ps1 README.md docs/build_guide.md docs/user_guide.md docs/superpowers/archive/windows-electron-helper-recovery.md
git commit -m "Switch desktop packaging docs and smoke tests to WebView"
```

Expected: package policy tests pass and active docs no longer describe Electron as the production desktop shell.

## Phase 7: Retire Electron Production Artifacts

**Files:**
- Modify: `webui/package.json`
- Modify: `webui/pnpm-lock.yaml`
- Move or delete: `webui/electron-builder.config.cjs`
- Move or delete: `webui/scripts/build-electron.cjs`
- Move or delete: `webui/scripts/run-electron-test.cjs`
- Move or delete: `webui/desktop/main`
- Move or delete: `webui/desktop/preload`
- Modify: `webui/host/__tests__/webview-package-policy.test.ts`

- [ ] **Step 1: Make retirement policy fail while Electron remains**

Extend `webui/host/__tests__/webview-package-policy.test.ts`:

```ts
it('retires Electron from production package dependencies and scripts', () => {
  const packageJson = JSON.parse(readFileSync(join(webuiRoot, 'package.json'), 'utf8'))
  assert.equal(packageJson.devDependencies.electron, undefined)
  assert.equal(packageJson.devDependencies['electron-builder'], undefined)
  assert.equal(packageJson.devDependencies['@types/electron'], undefined)
  assert.equal(packageJson.scripts['desktop:package'], undefined)
  assert.equal(packageJson.scripts['desktop:package:dir'], undefined)
  assert.equal(packageJson.scripts['desktop:build'], undefined)
  assert.equal(packageJson.main, undefined)
})
```

Run:

```powershell
cd webui
pnpm exec node scripts/run-electron-test.cjs host/__tests__/webview-package-policy.test.ts
```

Expected before cleanup: FAIL because Electron dependencies and scripts still exist.

- [ ] **Step 2: Remove production Electron scripts and dependencies**

`webui/package.json` should keep:

```json
"scripts": {
  "dev": "vite",
  "dev:desktop": "vite --port 8288 --strictPort",
  "build": "vue-tsc -b && vite build",
  "webview:compile": "cross-env ECNUVPN_RENDERER_TARGET=webview pnpm run build",
  "webview:package": "python ../scripts/package_ui_shell.py",
  "prepare:native": "node scripts/prepare-native.cjs",
  "test:contract": "node scripts/run-host-test.cjs desktop/main/__tests__/desktop-contract-generated.test.ts",
  "preview": "vite preview"
}
```

Remove:

- `electron`;
- `electron-builder`;
- `@types/electron`;
- `main`;
- `pnpm.onlyBuiltDependencies` entries for Electron.

Create `webui/scripts/run-host-test.cjs` as the neutral replacement for `run-electron-test.cjs` and update test commands to use it.

- [ ] **Step 3: Remove or archive Electron adapter source**

If the adapter is no longer needed, delete:

```text
webui/electron-builder.config.cjs
webui/scripts/build-electron.cjs
webui/desktop/main
webui/desktop/preload
```

If a short-lived diagnostic adapter must remain, move it under:

```text
webui/dev-electron/
```

and ensure production scripts do not reference it.

- [ ] **Step 4: Refresh lockfile, run web tests, and commit**

```powershell
cd webui
pnpm install --lockfile-only
pnpm exec node scripts/run-host-test.cjs host/__tests__/host-boundary.test.ts host/__tests__/webview-package-policy.test.ts
pnpm exec tsc -p tsconfig.json --noEmit
cd ..
git add webui/package.json webui/pnpm-lock.yaml webui/scripts webui/host webui/desktop webui/dev-electron webui/electron-builder.config.cjs
git commit -m "Retire Electron production desktop packaging"
```

Expected: lockfile no longer contains Electron package entries needed only by production desktop packaging, and production scripts no longer invoke `electron-builder`.

## Phase 8: Final Cross-Platform Acceptance

**Files:**
- Modify: `docs/superpowers/specs/2026-06-15-cross-platform-webview-shell-design.md`
- Modify: `docs/superpowers/plans/2026-06-15-cross-platform-webview-shell-migration-plan.md`
- Create: `docs/superpowers/reports/2026-06-16-webview-shell-acceptance-report.md`

- [ ] **Step 1: Run repository-level verification**

Windows:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\accept-webview-shell-windows.ps1
```

macOS:

```bash
ssh macmini "cd /Users/tomli/Development/Projects/CPP/ECNU-VPN && bash scripts/accept-webview-shell-macos.sh"
```

Linux:

```bash
bash scripts/accept-webview-shell-linux.sh
```

- [ ] **Step 2: Write acceptance report**

Create `docs/superpowers/reports/2026-06-16-webview-shell-acceptance-report.md`:

```markdown
# WebView Shell Acceptance Report

## Verification Matrix

| Platform | Configure | Build | CTest | Package | Smoke | Result |
| --- | --- | --- | --- | --- | --- | --- |
| Windows | `build/webview-acceptance/windows/configure.log` exit 0 | `build/webview-acceptance/windows/build.log` exit 0 | `build/webview-acceptance/windows/ctest.log` exit 0 | `build/webview-acceptance/windows/package.log` exit 0 | `build/webview-acceptance/windows/smoke.log` exit 0 | PASS |
| macOS | `build/webview-acceptance/macos/configure.log` exit 0 | `build/webview-acceptance/macos/build.log` exit 0 | `build/webview-acceptance/macos/ctest.log` exit 0 | `build/webview-acceptance/macos/package.log` exit 0 | `build/webview-acceptance/macos/smoke.log` exit 0 | PASS |
| Linux | `build/webview-acceptance/linux/configure.log` exit 0 | `build/webview-acceptance/linux/build.log` exit 0 | `build/webview-acceptance/linux/ctest.log` exit 0 | `build/webview-acceptance/linux/package.log` exit 0 | `build/webview-acceptance/linux/diff-check.log` exit 0 | PASS |

## Package Payload Check

- Windows package contains no `electron.exe` or `chromium.pak`.
- macOS package contains no `Electron Framework.framework` or `chromium.pak`.
- Linux package contains no Electron or Chromium payload.

## Manual Launch Check

- `exv-ui` loads packaged renderer.
- `status.get` reaches native core.
- `vpn.connect` request reaches native core/helper contract.
- core events are delivered back to renderer.
- closing the native window stops the owned core process.
```

If any command exits non-zero, record that row as `FAIL` and do not mark the migration accepted.

- [ ] **Step 3: Mark original docs with completion state**

At the top of the original migration plan, keep the current completion pointer:

```markdown
> Completion note: remaining work is tracked through
> `docs/superpowers/plans/2026-06-16-webview-shell-completion-plan.md`.
```

At the top of the design doc, keep the current implementation state:

```markdown
> Implementation status: Windows native WebView packaging has been verified in
> `docs/superpowers/reports/2026-06-16-webview-shell-acceptance-report.md`.
```

- [ ] **Step 4: Commit final acceptance**

```bash
git add docs/superpowers/specs/2026-06-15-cross-platform-webview-shell-design.md docs/superpowers/plans/2026-06-15-cross-platform-webview-shell-migration-plan.md docs/superpowers/reports/2026-06-16-webview-shell-acceptance-report.md
git commit -m "Document WebView shell acceptance"
```

## Final Acceptance Gates

The migration is complete only when all gates below are true:

- `src/platform/win32/ui_shell/webview2_host_win32.cpp` no longer returns `70` for supported production startup.
- `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm` no longer returns `70` for supported production startup.
- `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp` no longer returns `70` for supported production startup.
- `src/app/ui_shell/ui_shell_main.cpp` constructs a core transport, constructs a platform `UiWindow`, and calls `run_ui_shell_window`.
- Windows missing WebView2 Runtime path offers a controlled Evergreen install flow after explicit user consent.
- Default desktop package commands build WebView packages and never include Electron or Chromium payloads.
- `webui/package.json` production scripts and dependencies no longer include Electron or `electron-builder`.
- Active root and build docs describe WebView as the default desktop shell.
- Windows, macOS, and Linux acceptance scripts in Phase 8 have fresh passing output.
