# Logs Clear RPC Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `logs.clear` a formal generated contract action with native WebView host API coverage and a backend implementation that clears the persisted runtime log file.

**Architecture:** Treat `logs.clear` as a first-class logs action beside `logs.list` across the manifest, generated TypeScript/C++ contracts, native WebView bridges, and renderer host API. The backend truncates the existing runtime log file, not deletes the containing directory or moves paths. Because this is a persistent mutation, the RPC requires `confirm:true` and returns the existing `confirmation_required` error when confirmation is missing.

**Tech Stack:** C++20, nlohmann/json, CMake/CTest, generated contract manifest (`contracts/system.contract.json` + `scripts/generate_contracts.py`), TypeScript host contracts, Vue renderer, native Win32/WebView2 + Cocoa/WKWebView + GTK/WebKit bridge scripts.

---

## Current Evidence

- `contracts/system.contract.json` currently lists only `logs.list` in `surfaces.desktop_rpc.actions`, `surfaces.core_rpc.actions`, and `modules.logs.actions`.
- Generated artifacts currently expose only `logs.list`:
  - `src/contracts/generated/system_contract.hpp`
  - `webui/host/shared/generated/system-contract.ts`
  - `webui/desktop/shared/generated/system-contract.ts`
  - `contracts/generated/system_contract_snapshot.json`
- `webui/host/__tests__/desktop-contract-generated.test.ts` currently asserts that `logs.clear` is absent. This test must become the red-light check for formalizing it.
- `src/core/app_api/desktop_log_actions.cpp` registers only `logs.list`.
- `tests/app_api_logs_test.cpp` already describes desired `logs.clear` behavior, but CMake comments state this file is not compiled as a standalone target. Add a real release-gated test instead of relying on that orphaned coverage.
- Native WebView bridge scripts expose `window.ecnuVpn.logs.list(...)` only:
  - `src/platform/win32/ui_shell/webview2_host_win32.cpp`
  - `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
  - `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- `webui/src/api/host.ts` maps `GET /logs` to `logs.list()` only.
- `webui/src/pages/LogsPage.vue` has a "清空" button that only calls `vpn.clearLogs()`, so it clears renderer state but not the persisted log file.

## File Structure

- Modify `contracts/system.contract.json`: add `logs.clear` to desktop/core/logs module actions and core destructive actions; add `log_clear_failed` to standard core error codes.
- Regenerate:
  - `src/contracts/generated/system_contract.hpp`
  - `webui/host/shared/generated/system-contract.ts`
  - `webui/desktop/shared/generated/system-contract.ts`
  - `contracts/generated/system_contract_snapshot.json`
- Modify `webui/host/__tests__/desktop-contract-generated.test.ts`: require `logs.clear` in generated surfaces.
- Modify `tests/contract_manifest_test.cpp`: require generated C++ helpers to recognize `logs.clear` and `log_clear_failed`.
- Modify `tests/ui_shell_contract_test.cpp`: require native host bridge allowlist forwarding for `logs.clear`.
- Create `tests/core_api/desktop_log_actions_test.cpp`: executable backend regression for confirmation, truncation, and post-clear list behavior.
- Modify `CMakeLists.txt`: add `desktop_log_actions_test`, link it to `exv-core`, and label it `release-blocking` + `ui-contract`.
- Modify `src/core/app_api/desktop_log_actions.cpp`: implement and register `logs.clear`.
- Modify `src/feedback/feedback.hpp` and `src/feedback/feedback.cpp`: recognize `log_clear_failed` as a canonical, recoverable code.
- Modify native WebView bridge scripts in Win32/macOS/Linux host files to expose `logs.clear(confirm)`.
- Modify `webui/src/types/ecnu-vpn.d.ts`: add the `logs.clear(confirm: boolean)` host API type.
- Modify `webui/host/shared/host-contract.ts`: add `logsClear: '/logs/clear'`.
- Modify `webui/src/api/host.ts`: map `POST /logs/clear` to `window.ecnuVpn.logs.clear(confirm)`.
- Modify `webui/src/pages/LogsPage.vue`: make the clear button call the persisted clear RPC after confirmation.

---

### Task 1: Formalize `logs.clear` in Generated Contracts

**Files:**
- Modify: `webui/host/__tests__/desktop-contract-generated.test.ts`
- Modify: `tests/contract_manifest_test.cpp`
- Modify: `tests/ui_shell_contract_test.cpp`
- Modify: `contracts/system.contract.json`
- Generated: `src/contracts/generated/system_contract.hpp`
- Generated: `webui/host/shared/generated/system-contract.ts`
- Generated: `webui/desktop/shared/generated/system-contract.ts`
- Generated: `contracts/generated/system_contract_snapshot.json`

- [ ] **Step 1: Update TypeScript generated-contract test first**

Replace the existing test named `keeps persisted log mutation out of the desktop RPC surface` in `webui/host/__tests__/desktop-contract-generated.test.ts` with:

```ts
  it('exposes persisted log clearing through the generated RPC contract', () => {
    const desktopActions: readonly string[] = DESKTOP_RPC_ACTIONS
    const coreActions: readonly string[] = CORE_RPC_ACTIONS

    assert.ok(desktopActions.includes('logs.list'))
    assert.ok(desktopActions.includes('logs.clear'))
    assert.ok(coreActions.includes('logs.list'))
    assert.ok(coreActions.includes('logs.clear'))
    assert.ok(DESTRUCTIVE_CORE_RPC_ACTIONS.includes('logs.clear'))
    assert.ok(STANDARD_ERROR_CODES.includes('log_clear_failed'))
    assert.deepEqual(manifest().modules.logs.actions, ['logs.list', 'logs.clear'])
  })
```

- [ ] **Step 2: Update C++ contract tests first**

In `tests/contract_manifest_test.cpp`, add checks near the existing generated action checks:

```cpp
  ok = expect(exv::contracts::generated::is_desktop_rpc_action("logs.clear"),
              "desktop action logs.clear must be generated") &&
       ok;
  ok = expect(exv::contracts::generated::is_core_rpc_action("logs.clear"),
              "core action logs.clear must be generated") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_destructive_core_rpc_action(
               "logs.clear"),
           "logs.clear must require explicit confirmation") &&
       ok;
  ok = expect(
           exv::contracts::generated::is_standard_error_code(
               "log_clear_failed"),
           "log_clear_failed must be a generated standard error") &&
       ok;
```

In `tests/ui_shell_contract_test.cpp`, add an explicit Release-safe allowlist check near the other `is_allowed_host_action(...)` checks:

```cpp
  if (!is_allowed_host_action("logs.clear")) {
    return 1;
  }
```

Also add a forwarding regression near the existing `handle_host_request(...)` forwarding checks:

```cpp
  {
    bool core_invoked = false;
    const std::string forwarded = handle_host_request(
        R"({"id":8,"action":"logs.clear","payload":{"confirm":true}})",
        [&](const CoreRpcRequest &request) {
          core_invoked = true;
          if (request.action != "logs.clear") {
            return CoreRpcResponse{};
          }
          if (request.payload_json != R"({"confirm":true})") {
            return CoreRpcResponse{};
          }
          CoreRpcResponse response;
          response.id = 8;
          response.ok = true;
          response.data_json = R"({"ok":true})";
          return response;
        });
    if (!core_invoked) {
      return 1;
    }
    if (forwarded != R"({"id":8,"ok":true,"data":{"ok":true}})") {
      return 1;
    }
  }
```

- [ ] **Step 3: Run red-light checks**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
cmake --build build-windows\cpp --config Release --target contract_manifest_test ui_shell_contract_test
ctest --test-dir build-windows\cpp -C Release --output-on-failure -R "(contract_manifest_test|ui_shell_contract_test)"
```

Expected:

- `desktop-contract-generated.test.ts` fails because manifest/generated artifacts still omit `logs.clear`.
- `contract_manifest_test` fails because generated C++ omits `logs.clear` and `log_clear_failed`.
- `ui_shell_contract_test` fails because `logs.clear` is not in the generated allowlist yet.

- [ ] **Step 4: Update canonical manifest**

In `contracts/system.contract.json`:

Add `logs.clear` after `logs.list` in `surfaces.desktop_rpc.actions`:

```json
        "logs.list",
        "logs.clear"
```

Add `logs.clear` after `logs.list` in `surfaces.core_rpc.actions`:

```json
        "logs.list",
        "logs.clear",
```

Add `logs.clear` to `surfaces.core_rpc.destructive_actions`:

```json
      "destructive_actions": [
        "config.reset",
        "key.reset",
        "maintenance.killStaleCore",
        "logs.clear"
      ],
```

Add `log_clear_failed` to `surfaces.core_rpc.error_codes` after `key_corrupt`:

```json
        "key_corrupt",
        "log_clear_failed"
```

Update `modules.logs.actions`:

```json
    "logs": {
      "shallow": true,
      "namespace": "logs.",
      "actions": ["logs.list", "logs.clear"]
    }
```

- [ ] **Step 5: Regenerate checked-in contract artifacts**

Run:

```powershell
python scripts\generate_contracts.py
python scripts\generate_contracts.py --check
```

Expected:

- `--check` exits `0`.
- The generated C++/TS arrays include `logs.clear`.
- `contracts/generated/system_contract_snapshot.json` matches `contracts/system.contract.json`.

- [ ] **Step 6: Run green contract checks**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts
cmake --build build-windows\cpp --config Release --target contract_manifest_test ui_shell_contract_test
ctest --test-dir build-windows\cpp -C Release --output-on-failure -R "(contract_manifest_test|ui_shell_contract_test)"
```

Expected: all commands exit `0`.

- [ ] **Step 7: Commit Task 1**

Run:

```powershell
git add contracts\system.contract.json contracts\generated\system_contract_snapshot.json src\contracts\generated\system_contract.hpp webui\host\shared\generated\system-contract.ts webui\desktop\shared\generated\system-contract.ts webui\host\__tests__\desktop-contract-generated.test.ts tests\contract_manifest_test.cpp tests\ui_shell_contract_test.cpp
git commit -m "contract: add logs.clear rpc action"
```

---

### Task 2: Implement Backend Log Clearing

**Files:**
- Create: `tests/core_api/desktop_log_actions_test.cpp`
- Modify: `CMakeLists.txt`
- Modify: `src/core/app_api/desktop_log_actions.cpp`
- Modify: `src/feedback/feedback.hpp`
- Modify: `src/feedback/feedback.cpp`

- [ ] **Step 1: Write backend regression test**

Create `tests/core_api/desktop_log_actions_test.cpp`:

```cpp
#include "core/app_api/app_api.hpp"
#include "platform/common/runtime_paths.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

struct RuntimePathGuard {
  ~RuntimePathGuard() { ecnuvpn::platform::clear_runtime_path_override(); }
};

std::string read_file(const std::filesystem::path &path) {
  std::ifstream in(path.string(), std::ios::in);
  return std::string((std::istreambuf_iterator<char>(in)),
                     std::istreambuf_iterator<char>());
}

} // namespace

int main() {
  RuntimePathGuard guard;

  const auto temp_root =
      std::filesystem::temp_directory_path() /
      ("ecnuvpn-desktop-log-actions-test-" +
       std::to_string(
           std::chrono::steady_clock::now().time_since_epoch().count()));
  std::filesystem::create_directories(temp_root);
  const auto log_path = temp_root / "ecnuvpn.log";
  {
    std::ofstream out(log_path.string(), std::ios::out | std::ios::trunc);
    out << "[INFO] first marker\n";
    out << "[ERROR] second marker\n";
  }

  const nlohmann::json base_payload{
      {"home", temp_root.string()},
      {"config_dir", temp_root.string()},
      {"lines", 100},
  };

  bool ok = true;

  const auto before =
      ecnuvpn::app_api::handle_action("logs.list", base_payload);
  ok = expect(before.is_array() && before.size() == 2,
              "logs.list should read existing runtime log lines") &&
       ok;

  const auto denied =
      ecnuvpn::app_api::handle_action("logs.clear", base_payload);
  ok = expect(denied.is_object() && denied.value("ok", true) == false,
              "logs.clear without confirm should fail") &&
       ok;
  ok = expect(denied.value("code", std::string()) == "confirmation_required",
              "logs.clear without confirm should return confirmation_required") &&
       ok;
  ok = expect(!read_file(log_path).empty(),
              "logs.clear without confirm must not truncate the file") &&
       ok;

  nlohmann::json confirmed_payload = base_payload;
  confirmed_payload["confirm"] = true;
  const auto cleared =
      ecnuvpn::app_api::handle_action("logs.clear", confirmed_payload);
  ok = expect(cleared.is_object() && cleared.value("ok", false),
              "confirmed logs.clear should succeed") &&
       ok;
  ok = expect(read_file(log_path).empty(),
              "confirmed logs.clear should truncate runtime log file") &&
       ok;

  const auto after =
      ecnuvpn::app_api::handle_action("logs.list", confirmed_payload);
  ok = expect(after.is_array() && after.empty(),
              "logs.list should be empty after confirmed clear") &&
       ok;

  std::filesystem::remove_all(temp_root);

  if (ok) {
    std::cout << "desktop_log_actions_test: all assertions passed\n";
  } else {
    std::cerr << "desktop_log_actions_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
```

- [ ] **Step 2: Add the test target**

In `CMakeLists.txt`, add the target near the other core API action tests:

```cmake
add_executable(desktop_log_actions_test
    tests/core_api/desktop_log_actions_test.cpp
)

target_include_directories(desktop_log_actions_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

target_link_libraries(desktop_log_actions_test PRIVATE exv-core)

add_test(NAME desktop_log_actions_test COMMAND desktop_log_actions_test)
```

Add `desktop_log_actions_test` to `_release_blocking_tests` near `app_api_rpc_dispatcher_test`, and to the UI/RPC contract label block near `app_api_rpc_dispatcher_test`.

- [ ] **Step 3: Run red-light backend test**

Run:

```powershell
cmake --build build-windows\cpp --config Release --target desktop_log_actions_test
ctest --test-dir build-windows\cpp -C Release --output-on-failure -R desktop_log_actions_test
```

Expected: test fails because `logs.clear` is not registered yet.

- [ ] **Step 4: Add canonical feedback code**

In `src/feedback/feedback.hpp`, add:

```cpp
inline constexpr const char *kLogClearFailed = "log_clear_failed";
```

In `src/feedback/feedback.cpp`, update `is_canonical(...)`:

```cpp
         value == code::kInvalidRequest ||
         value == code::kLogClearFailed ||
         value == code::kConnectionFailed;
```

Update `info_for(...)` before the generic fallback:

```cpp
  if (canonical == code::kLogClearFailed)
    return {canonical, true,
            "Close other ECNU VPN processes if they are writing logs, then retry."};
```

- [ ] **Step 5: Implement `logs.clear` backend**

In `src/core/app_api/desktop_log_actions.cpp`, add includes:

```cpp
#include <filesystem>
```

Add a helper in the anonymous namespace:

```cpp
nlohmann::json clear_logs_json(const nlohmann::json &payload) {
  apply_desktop_runtime_context(payload);

  if (!payload.value("confirm", false)) {
    return error("logs.clear requires confirm:true", "confirmation_required");
  }

  platform::ensure_dir(platform::get_config_dir());
  ecnuvpn::platform::logging::configure_default_logging(false);

  const std::string log_path = runtime::paths().log_path;
  const std::filesystem::path path(log_path);
  const auto parent = path.parent_path();
  if (!parent.empty()) {
    std::filesystem::create_directories(parent);
  }

  std::ofstream out(log_path, std::ios::out | std::ios::trunc);
  if (!out.is_open()) {
    return error("Unable to clear runtime log file", "log_clear_failed");
  }
  out.close();
  if (!out.good()) {
    return error("Unable to clear runtime log file", "log_clear_failed");
  }
  if (!platform::sync_owner(log_path)) {
    return error("Unable to restore runtime log file ownership",
                 "log_clear_failed");
  }
  return nlohmann::json{{"ok", true}};
}
```

Then register the action:

```cpp
  adapter.register_legacy_handler(
      "logs.clear", [](const nlohmann::json &payload) -> nlohmann::json {
        return clear_logs_json(payload);
      });
```

Keep `logs_json(...)` behavior unchanged.

- [ ] **Step 6: Run green backend tests**

Run:

```powershell
cmake --build build-windows\cpp --config Release --target desktop_log_actions_test app_api_rpc_dispatcher_test
ctest --test-dir build-windows\cpp -C Release --output-on-failure -R "(desktop_log_actions_test|app_api_rpc_dispatcher_test)"
```

Expected: all tests pass.

- [ ] **Step 7: Commit Task 2**

Run:

```powershell
git add tests\core_api\desktop_log_actions_test.cpp CMakeLists.txt src\core\app_api\desktop_log_actions.cpp src\feedback\feedback.hpp src\feedback\feedback.cpp
git commit -m "core: implement logs.clear action"
```

---

### Task 3: Expose `logs.clear` Through Native Host APIs

**Files:**
- Modify: `webui/src/types/ecnu-vpn.d.ts`
- Modify: `webui/host/shared/host-contract.ts`
- Modify: `webui/src/api/host.ts`
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Modify: `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- Modify: `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- Modify: `webui/host/__tests__/host-boundary.test.ts`

- [ ] **Step 1: Add host API tests first**

In `webui/host/__tests__/host-boundary.test.ts`, add:

```ts
describe('logs clear host API', () => {
  it('declares a neutral persisted log clear path', async () => {
    const contract = await import('../shared/host-contract.ts')

    assert.equal(contract.desktopApiPaths.logs, '/logs')
    assert.equal(contract.desktopApiPaths.logsClear, '/logs/clear')
  })

  it('keeps native bridge declarations in sync for logs.clear', () => {
    const repoRoot = join(webuiRoot, '..')
    const checked = [
      join(repoRoot, 'src/platform/win32/ui_shell/webview2_host_win32.cpp'),
      join(repoRoot, 'src/platform/darwin/ui_shell/wk_webview_host_darwin.mm'),
      join(repoRoot, 'src/platform/linux/ui_shell/webkitgtk_host_linux.cpp'),
    ]

    for (const file of checked) {
      const text = readFileSync(file, 'utf8')
      assert.match(text, /logs:\s*\{/)
      assert.match(text, /clear:\s*\(confirm\)\s*=>\s*rpc\('logs\.clear',\s*\{\s*confirm\s*\}\)/)
    }
  })
})
```

- [ ] **Step 2: Run red-light host tests**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/host-boundary.test.ts
```

Expected: fails because `/logs/clear` and native bridge declarations are missing.

- [ ] **Step 3: Add neutral host contract path**

In `webui/host/shared/host-contract.ts`, add:

```ts
  logsClear: '/logs/clear',
```

immediately after:

```ts
  logs: '/logs',
```

- [ ] **Step 4: Add typed `window.ecnuVpn.logs.clear`**

In `webui/src/types/ecnu-vpn.d.ts`, update:

```ts
  logs: {
    list(options?: { lines?: number; filter?: string }): Promise<LogEntry[]>
    clear(confirm: boolean): Promise<{ ok: true }>
  }
```

- [ ] **Step 5: Map host API POST path**

In `webui/src/api/host.ts`, add a case in `post(...)`:

```ts
      case desktopApiPaths.logsClear:
        return wrap(
          window.ecnuVpn!.logs.clear(plainPayload(body)?.confirm ?? false),
        ) as ApiResponse<T>
```

- [ ] **Step 6: Add native bridge methods**

In each native bridge script, replace the one-line logs declaration:

```js
logs: { list: (options) => rpc('logs.list', options ?? {}) },
```

with:

```js
logs: {
  list: (options) => rpc('logs.list', options ?? {}),
  clear: (confirm) => rpc('logs.clear', { confirm }),
},
```

Apply this to:

- `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`

- [ ] **Step 7: Run green host tests**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/host-boundary.test.ts host/__tests__/desktop-contract-generated.test.ts
```

Expected: all tests pass.

- [ ] **Step 8: Commit Task 3**

Run:

```powershell
git add webui\src\types\ecnu-vpn.d.ts webui\host\shared\host-contract.ts webui\src\api\host.ts src\platform\win32\ui_shell\webview2_host_win32.cpp src\platform\darwin\ui_shell\wk_webview_host_darwin.mm src\platform\linux\ui_shell\webkitgtk_host_linux.cpp webui\host\__tests__\host-boundary.test.ts
git commit -m "ui-shell: expose logs.clear host api"
```

---

### Task 4: Wire Logs Page Clear Button to Persisted RPC

**Files:**
- Modify: `webui/src/pages/LogsPage.vue`

- [ ] **Step 1: Write a focused renderer behavior test if a local pattern exists**

First inspect whether this repo has Vue component tests:

```powershell
rg -n "mount\\(|@vue/test-utils|LogsPage" webui tests
```

If there is no component-test harness, do not add a new framework. Use TypeScript build plus host tests in Step 5 for this task.

- [ ] **Step 2: Replace local-only clear with confirmed persisted clear**

In `webui/src/pages/LogsPage.vue`, replace:

```ts
function clearLogs() {
  vpn.clearLogs()
}
```

with:

```ts
function clearLogs() {
  ui.requestConfirm('确认清空当前日志文件？此操作不可撤销。', async () => {
    try {
      const api = (await import('../api/host')).default
      await api.post<{ ok: true }>('/logs/clear', { confirm: true })
      vpn.clearLogs()
      ui.addToast('日志已清空', 'success')
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      ui.requestError({ title: '清空日志失败', message })
    }
  })
}
```

Keep the existing button and icon.

- [ ] **Step 3: Confirm no local-only clear path remains in LogsPage**

Run:

```powershell
Select-String -Path webui\src\pages\LogsPage.vue -Pattern "vpn.clearLogs\\(\\)" -Context 3,3
```

Expected: the only match is inside the successful `api.post('/logs/clear', { confirm: true })` branch.

- [ ] **Step 4: Run TypeScript/Vite build**

Run:

```powershell
pnpm --dir webui run build
```

Expected: build exits `0`.

- [ ] **Step 5: Commit Task 4**

Run:

```powershell
git add webui\src\pages\LogsPage.vue
git commit -m "webui: clear persisted logs from logs page"
```

---

### Task 5: Full Verification

**Files:**
- No source files expected unless verification exposes a defect.

- [ ] **Step 1: Run contract generation check**

Run:

```powershell
python scripts\generate_contracts.py --check
```

Expected: exits `0`.

- [ ] **Step 2: Run host contract tests**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts host/__tests__/host-boundary.test.ts host/__tests__/webview-package-policy.test.ts
```

Expected: all host tests pass.

- [ ] **Step 3: Run focused C++ tests**

Run:

```powershell
cmake --build build-windows\cpp --config Release --target contract_manifest_test ui_shell_contract_test desktop_log_actions_test win32_webview2_runtime_test
ctest --test-dir build-windows\cpp -C Release --output-on-failure -R "(contract_manifest_test|ui_shell_contract_test|desktop_log_actions_test|win32_webview2_runtime_test)"
```

Expected: all tests pass.

- [ ] **Step 4: Run WebUI build**

Run:

```powershell
pnpm --dir webui run build
```

Expected: TypeScript and Vite build pass.

- [ ] **Step 5: Run Windows desktop build**

Run:

```powershell
powershell.exe -NoProfile -NoLogo -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop
```

Expected:

- WebView renderer builds.
- `exv-ui.exe` builds.
- release-blocking CTest set passes.
- package is generated under `build\windows\webview\package\ECNU VPN`.

- [ ] **Step 6: Diff hygiene**

Run:

```powershell
git diff --check
git status --short
```

Expected:

- `git diff --check` exits `0`.
- `git status --short` is clean after all task commits.

- [ ] **Step 7: Manual smoke note**

Manual UI smoke is recommended after packaging:

1. Launch native WebView package.
2. Open Logs page.
3. Confirm visible log lines exist.
4. Click "清空".
5. Confirm the prompt.
6. Verify the page becomes empty.
7. Restart app and verify old cleared lines do not reappear.

If manual smoke is not run, report it as a verification gap.

---

## Acceptance Criteria

- `logs.clear` appears in:
  - `contracts/system.contract.json`
  - `src/contracts/generated/system_contract.hpp`
  - both generated TypeScript contract files
  - `contracts/generated/system_contract_snapshot.json`
- `logs.clear` is accepted by `is_allowed_host_action(...)`.
- `logs.clear` is registered in the backend desktop action registry via `register_desktop_log_actions(...)`.
- `logs.clear` without `confirm:true` returns `ok:false` with code `confirmation_required`.
- `logs.clear` with `confirm:true` truncates the runtime log file and returns `{"ok":true}`.
- `logs.list` returns an empty array after a confirmed clear of the same runtime path.
- Native WebView `window.ecnuVpn.logs.clear(confirm)` exists on Windows, macOS, and Linux bridge scripts.
- `POST /logs/clear` exists in the neutral host API.
- Logs page clear button clears persisted logs after confirmation, not only renderer state.
- New backend coverage is part of a CTest target and is release-blocking.

## Notes for Implementers

- Do not delete the log file or log directory; truncate the file. `FileLogSink` opens the file on each write, so truncation is compatible with later logging.
- Keep `logs.list` payload shape unchanged.
- Use `apply_desktop_runtime_context(payload)` before resolving paths so tests and elevated/packaged flows use the intended runtime state directory.
- Avoid adding a new frontend dependency or component-test framework just for this feature. If there is no existing Vue component test harness, rely on TypeScript build plus host/API/backend tests.
- Do not edit generated contract files manually. Change `contracts/system.contract.json`, then run `python scripts\generate_contracts.py`.
