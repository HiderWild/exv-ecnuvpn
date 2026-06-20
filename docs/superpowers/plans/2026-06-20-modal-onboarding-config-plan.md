# Modal Onboarding Config Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build core-owned first-run quick start, missing-credential completion, clipped compact modal styling, and readable settings selects.

**Architecture:** Core owns configuration validity and emits a typed `quick-start-request` event after repairing missing or incomplete config. The UI shell forwards that event to Vue, and Vue renders all in-window dialogs through one compact modal foundation mounted inside `AppWindowFrame` so scrims are clipped by the content surface and never cover the shadow gutter. Credential completion is moved from a password-only prompt to a typed username/password dialog that persists username and optionally stores password exactly like settings.

**Tech Stack:** C++20/CMake/CTest, nlohmann-json, generated contract scripts, Vue 3 + Pinia + TypeScript + Vite, Node `node:test`, Tailwind utility classes, lucide-vue-next icons.

---

## Scope Notes

This is one plan because the event contract, core config initialization, renderer quick start, and modal foundation all depend on the same first-run lifecycle. Execute in order; later frontend tasks expect the generated event type and core initialization API created in the first tasks.

Before executing Task 1, start from a clean isolated work area using `superpowers:using-git-worktrees` or the existing branch policy for this repository.

## File Map

Create:

- `src/core/config/config_initialization.hpp`: public startup repair API and quick-start event payload helpers.
- `src/core/config/config_initialization.cpp`: raw JSON completeness/type validation plus default config rewrite.
- `tests/config_initialization_test.cpp`: focused C++ tests for missing, invalid, incomplete, complete config.
- `webui/src/components/ModalShell.vue`: compact in-window modal foundation.
- `webui/src/components/CredentialPromptDialog.vue`: username/password completion dialog with remember-password checkbox.
- `webui/src/components/QuickStartDialog.vue`: quick/custom onboarding dialog and import entry point.
- `webui/src/components/ClosePromptDialog.vue`: extracted close-app prompt using the shared shell.
- `webui/host/__tests__/modal-onboarding-contract.test.ts`: static renderer contract tests for modal usage, quick start, credentials, select readability.

Modify:

- `contracts/system.contract.json`: add `quick-start-request` desktop event.
- `contracts/generated/system_contract_snapshot.json`: regenerated snapshot.
- `src/contracts/generated/system_contract.hpp`: regenerated C++ contract.
- `webui/host/shared/generated/system-contract.ts`: regenerated host contract.
- `webui/desktop/shared/generated/system-contract.ts`: regenerated desktop contract.
- `tests/contract_manifest_test.cpp`: assert generated event includes `quick-start-request`.
- `webui/host/__tests__/desktop-contract-generated.test.ts`: assert generated TS event includes `quick-start-request`.
- `CMakeLists.txt`: add config initialization source and test target.
- `src/core/config/config_manager.cpp`: load through the new initialization repair helper.
- `src/core/config/config_persistence.cpp`: CLI load path uses the same initialization repair helper.
- `src/core/core_process.cpp`: run startup config initialization and emit `quick-start-request` in stdin/UI mode.
- `tests/core_process_lifecycle_test.cpp`: make valid config helper complete and test startup quick-start event emission.
- `src/app/ui_shell/core_rpc_client.cpp`: retain quick-start events until a handler is installed.
- `src/app/ui_shell/core_rpc_client.hpp`: add event backlog fields if not already present.
- `src/app/ui_shell/ui_shell_runtime.cpp`: forward retained quick-start event to the window after the event handler is attached.
- `webui/src/types/exv.d.ts`: add typed quick-start event payload.
- `webui/src/composables/useSSE.ts`: handle `quick-start-request`.
- `webui/src/stores/ui.ts`: add quick-start and credential prompt state/actions.
- `webui/src/stores/vpn.ts`: replace password-only connect resolution with credential completion.
- `webui/src/stores/config.ts`: remove frontend ownership of `service_install_prompt_seen`; keep only local minimal-mode overlay.
- `webui/src/App.vue`: remove frontend service prompt logic, mount all modal consumers inside `AppWindowFrame`.
- `webui/src/components/ErrorDialog.vue`, `ConfirmDialog.vue`, `AuthContinuationDialog.vue`, `PasswordPromptDialog.vue`, `CoreCrashed.vue`, `ServiceInstallLoadingOverlay.vue`: convert to `ModalShell` or delegate legacy password usage to `CredentialPromptDialog`.
- `webui/src/pages/ServiceInstallPromptModal.vue`: apply compact native modal tokens while it remains available for native modal routes.
- `webui/src/pages/SettingsPage.vue`: add readable select option class to native selects.
- `webui/src/style.css`: global select option colors and shared modal tokens.
- `webui/package.json`: include the new host contract test in `test:host`.

## Task 1: Add Generated Quick-Start Event Contract

**Files:**

- Modify: `contracts/system.contract.json`
- Modify: `contracts/generated/system_contract_snapshot.json`
- Modify: `src/contracts/generated/system_contract.hpp`
- Modify: `webui/host/shared/generated/system-contract.ts`
- Modify: `webui/desktop/shared/generated/system-contract.ts`
- Modify: `tests/contract_manifest_test.cpp`
- Modify: `webui/host/__tests__/desktop-contract-generated.test.ts`

- [ ] **Step 1: Write failing contract assertions**

In `tests/contract_manifest_test.cpp`, add this assertion near the other `DESKTOP_RPC_*` assertions:

```cpp
  ok = expect(contains(exv::contracts::generated::DESKTOP_RPC_EVENT_TYPES,
                       "quick-start-request"),
              "desktop RPC event types must include quick-start-request") &&
       ok;
```

In `webui/host/__tests__/desktop-contract-generated.test.ts`, extend the `keeps desktop event and error constants aligned with the public desktop contract` test with:

```ts
    expectContains(DESKTOP_RPC_EVENT_TYPES, 'quick-start-request')
    expectContains(desktopEventTypes, 'quick-start-request')
```

- [ ] **Step 2: Run the focused failing tests**

Run:

```powershell
pnpm --dir webui test:contract
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug -R "^contract_manifest_test$" --output-on-failure
```

Expected: the TypeScript test fails because the generated event array does not include `quick-start-request`; the C++ test fails with the new assertion after the target builds.

- [ ] **Step 3: Add the event to the manifest**

In `contracts/system.contract.json`, add `quick-start-request` to `surfaces.desktop_rpc.event_types` after `core-crashed`:

```json
[
  "log",
  "status",
  "heartbeat",
  "service-progress",
  "close-request",
  "core-crashed",
  "quick-start-request"
]
```

- [ ] **Step 4: Regenerate contract outputs**

Run:

```powershell
python scripts/generate_contracts.py
```

Expected: the generator updates:

```text
contracts/generated/system_contract_snapshot.json
src/contracts/generated/system_contract.hpp
webui/host/shared/generated/system-contract.ts
webui/desktop/shared/generated/system-contract.ts
```

- [ ] **Step 5: Verify contract tests pass**

Run:

```powershell
pnpm --dir webui test:contract
cmake --build build --target contract_manifest_test --config Debug
ctest --test-dir build -C Debug -R "^contract_manifest_test$" --output-on-failure
```

Expected: both test commands pass.

- [ ] **Step 6: Commit**

Run:

```powershell
git add contracts/system.contract.json contracts/generated/system_contract_snapshot.json src/contracts/generated/system_contract.hpp webui/host/shared/generated/system-contract.ts webui/desktop/shared/generated/system-contract.ts tests/contract_manifest_test.cpp webui/host/__tests__/desktop-contract-generated.test.ts
git commit -m "contract: add quick start desktop event"
```

## Task 2: Add Config Initialization Repair API

**Files:**

- Create: `src/core/config/config_initialization.hpp`
- Create: `src/core/config/config_initialization.cpp`
- Create: `tests/config_initialization_test.cpp`
- Modify: `src/core/config/config_manager.cpp`
- Modify: `src/core/config/config_persistence.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Write the failing config initialization test**

Create `tests/config_initialization_test.cpp`:

```cpp
#include "core/config/config_initialization.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

std::filesystem::path unique_temp_dir(const char *name) {
  const auto now = std::chrono::steady_clock::now().time_since_epoch().count();
  auto dir = std::filesystem::temp_directory_path() /
             (std::string(name) + "-" + std::to_string(now));
  std::filesystem::create_directories(dir);
  return dir;
}

std::filesystem::path config_path(const std::filesystem::path &dir) {
  return dir / "config.json";
}

void write_json(const std::filesystem::path &dir, const json &value) {
  std::ofstream out(config_path(dir), std::ios::out | std::ios::trunc);
  out << value.dump(2);
}

json read_json(const std::filesystem::path &dir) {
  std::ifstream in(config_path(dir));
  json parsed;
  in >> parsed;
  return parsed;
}

json complete_config() {
  return {
      {"server", "https://vpn-ct.ecnu.edu.cn"},
      {"username", ""},
      {"password", ""},
      {"mtu", 1290},
      {"useragent", "test-agent"},
      {"disable_dtls", false},
      {"remember_password", true},
      {"routes", json::array({"49.52.4.0/25"})},
      {"extra_args", json::array()},
      {"log_file", ""},
      {"vpn_engine", "native"},
      {"windows_tunnel_driver", "auto"},
      {"windows_tap_interface", ""},
      {"auto_reconnect", true},
      {"minimal_mode", false},
      {"service_install_prompt_seen", false},
      {"minimal_install_service_before_connect", true},
  };
}

bool has_minimal_required_fields(const json &cfg) {
  const char *fields[] = {
      "server",
      "username",
      "password",
      "mtu",
      "useragent",
      "disable_dtls",
      "remember_password",
      "routes",
      "extra_args",
      "log_file",
      "vpn_engine",
      "windows_tunnel_driver",
      "windows_tap_interface",
      "auto_reconnect",
      "minimal_mode",
      "service_install_prompt_seen",
      "minimal_install_service_before_connect",
  };
  for (const char *field : fields) {
    if (!cfg.contains(field)) {
      std::cerr << "missing field: " << field << '\n';
      return false;
    }
  }
  return true;
}

} // namespace

int main() {
  bool ok = true;

  {
    auto dir = unique_temp_dir("exv-config-init-missing");
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Missing,
                "missing config reports Missing") &&
         ok;
    ok = expect(result.should_request_quick_start(),
                "missing config requests quick start") &&
         ok;
    ok = expect(std::filesystem::exists(config_path(dir)),
                "missing config writes config.json") &&
         ok;
    ok = expect(has_minimal_required_fields(read_json(dir)),
                "missing config writes every minimal field") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  {
    auto dir = unique_temp_dir("exv-config-init-invalid-json");
    std::ofstream out(config_path(dir), std::ios::out | std::ios::trunc);
    out << "{not-json";
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Invalid,
                "parse failure reports Invalid") &&
         ok;
    ok = expect(result.should_request_quick_start(),
                "parse failure requests quick start") &&
         ok;
    ok = expect(has_minimal_required_fields(read_json(dir)),
                "parse failure replaces file with complete config") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  {
    auto dir = unique_temp_dir("exv-config-init-incomplete");
    write_json(dir, json{{"server", "https://vpn.example.invalid"},
                         {"username", ""},
                         {"password", ""}});
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Invalid,
                "missing initialized fields reports Invalid") &&
         ok;
    ok = expect(result.reason.find("missing:mtu") != std::string::npos,
                "missing field reason names mtu") &&
         ok;
    ok = expect(has_minimal_required_fields(read_json(dir)),
                "incomplete config is replaced with complete defaults") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  {
    auto dir = unique_temp_dir("exv-config-init-wrong-type");
    auto cfg = complete_config();
    cfg["mtu"] = "1290";
    write_json(dir, cfg);
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Invalid,
                "wrong JSON type reports Invalid") &&
         ok;
    ok = expect(result.reason.find("type:mtu") != std::string::npos,
                "wrong type reason names mtu") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  {
    auto dir = unique_temp_dir("exv-config-init-complete");
    write_json(dir, complete_config());
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Normal,
                "complete config reports Normal") &&
         ok;
    ok = expect(!result.should_request_quick_start(),
                "complete config does not request quick start") &&
         ok;
    ok = expect(read_json(dir).at("server") == "https://vpn-ct.ecnu.edu.cn",
                "complete config is not replaced") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  if (ok) {
    std::cout << "config_initialization_test: all assertions passed\n";
  } else {
    std::cerr << "config_initialization_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
```

- [ ] **Step 2: Add the test target and verify it fails to compile**

Add this target near `config_actions_test` or the adjacent config tests in `CMakeLists.txt`:

```cmake
add_executable(config_initialization_test
    tests/config_initialization_test.cpp
)

target_include_directories(config_initialization_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)

target_link_libraries(config_initialization_test PRIVATE exv-core)

add_test(NAME config_initialization_test COMMAND config_initialization_test)
```

Add `config_initialization_test` to the same label groups that already include `config_module_smoke_test` and `config_actions_test`.

Run:

```powershell
cmake --build build --target config_initialization_test --config Debug
```

Expected: compile fails because `core/config/config_initialization.hpp` does not exist.

- [ ] **Step 3: Add the public config initialization header**

Create `src/core/config/config_initialization.hpp`:

```cpp
#pragma once

#include "core/config/config.hpp"

#include <nlohmann/json.hpp>

#include <string>

namespace exv::config {

enum class ConfigInitializationStatus {
  Normal,
  Missing,
  Invalid,
};

struct ConfigInitializationResult {
  ConfigInitializationStatus status = ConfigInitializationStatus::Normal;
  Config config{};
  std::string reason = "normal";

  bool should_request_quick_start() const {
    return status == ConfigInitializationStatus::Missing ||
           status == ConfigInitializationStatus::Invalid;
  }
};

bool is_complete_initialized_config_json(const nlohmann::json &value,
                                         std::string *reason = nullptr);
ConfigInitializationResult ensure_initialized_config(
    const std::string &config_dir);
nlohmann::json quick_start_request_data(
    const ConfigInitializationResult &result);

} // namespace exv::config
```

- [ ] **Step 4: Add the implementation**

Create `src/core/config/config_initialization.cpp`:

```cpp
#include "core/config/config_initialization.hpp"

#include "observability/log_facade.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/path_utils.hpp"

#include <filesystem>
#include <string>
#include <string_view>

namespace exv::config {
namespace {

using json = nlohmann::json;

bool is_string_array(const json &value) {
  if (!value.is_array()) {
    return false;
  }
  for (const auto &item : value) {
    if (!item.is_string()) {
      return false;
    }
  }
  return true;
}

bool field_type_matches(std::string_view name, const json &value) {
  if (name == "server" || name == "username" || name == "password" ||
      name == "useragent" || name == "log_file" || name == "vpn_engine" ||
      name == "windows_tunnel_driver" || name == "windows_tap_interface") {
    return value.is_string();
  }
  if (name == "mtu") {
    return value.is_number_integer();
  }
  if (name == "disable_dtls" || name == "remember_password" ||
      name == "auto_reconnect" || name == "minimal_mode" ||
      name == "service_install_prompt_seen" ||
      name == "minimal_install_service_before_connect") {
    return value.is_boolean();
  }
  if (name == "routes" || name == "extra_args") {
    return is_string_array(value);
  }
  return false;
}

constexpr std::string_view kRequiredFields[] = {
    "server",
    "username",
    "password",
    "mtu",
    "useragent",
    "disable_dtls",
    "remember_password",
    "routes",
    "extra_args",
    "log_file",
    "vpn_engine",
    "windows_tunnel_driver",
    "windows_tap_interface",
    "auto_reconnect",
    "minimal_mode",
    "service_install_prompt_seen",
    "minimal_install_service_before_connect",
};

bool write_default_config(const std::string &config_dir, Config &out) {
  namespace fs = std::filesystem;
  std::error_code ec;
  fs::create_directories(fs::u8path(config_dir), ec);

  out = Config{};
  normalize_native_only(out);

  const std::string final_path = platform::config_path(config_dir);
  const std::string tmp_path = final_path + ".tmp";
  const json serialized = out;

  if (!platform::write_file(tmp_path, serialized.dump(4))) {
    exv::observability::LogFacade::error(
        "ensure_initialized_config: failed to write temp config: " + tmp_path);
    return false;
  }

  fs::remove(fs::u8path(final_path), ec);
  ec.clear();
  fs::rename(fs::u8path(tmp_path), fs::u8path(final_path), ec);
  if (!ec) {
    return true;
  }

  ec.clear();
  fs::copy_file(fs::u8path(tmp_path), fs::u8path(final_path),
                fs::copy_options::overwrite_existing, ec);
  fs::remove(fs::u8path(tmp_path), ec);
  if (ec) {
    exv::observability::LogFacade::error(
        "ensure_initialized_config: failed to replace config: " + ec.message());
    return false;
  }
  return true;
}

ConfigInitializationResult repaired_result(ConfigInitializationStatus status,
                                           const std::string &reason,
                                           const std::string &config_dir) {
  Config cfg;
  (void)write_default_config(config_dir, cfg);
  return ConfigInitializationResult{status, cfg, reason};
}

const char *status_reason(ConfigInitializationStatus status) {
  switch (status) {
  case ConfigInitializationStatus::Missing:
    return "missing";
  case ConfigInitializationStatus::Invalid:
    return "invalid";
  case ConfigInitializationStatus::Normal:
  default:
    return "normal";
  }
}

} // namespace

bool is_complete_initialized_config_json(const json &value,
                                         std::string *reason) {
  if (!value.is_object()) {
    if (reason) {
      *reason = "type:root";
    }
    return false;
  }

  for (const auto field : kRequiredFields) {
    const std::string field_name(field);
    if (!value.contains(field_name)) {
      if (reason) {
        *reason = "missing:" + field_name;
      }
      return false;
    }
    if (!field_type_matches(field, value.at(field_name))) {
      if (reason) {
        *reason = "type:" + field_name;
      }
      return false;
    }
  }
  if (reason) {
    *reason = "normal";
  }
  return true;
}

ConfigInitializationResult ensure_initialized_config(
    const std::string &config_dir) {
  const std::string path = platform::config_path(config_dir);
  if (!platform::file_exists(path)) {
    return repaired_result(ConfigInitializationStatus::Missing, "missing",
                           config_dir);
  }

  try {
    const auto parsed = json::parse(platform::read_file(path));
    std::string completeness_reason;
    if (!is_complete_initialized_config_json(parsed, &completeness_reason)) {
      exv::observability::LogFacade::warn(
          "ensure_initialized_config: invalid config completeness: " +
          completeness_reason);
      return repaired_result(ConfigInitializationStatus::Invalid,
                             completeness_reason, config_dir);
    }

    Config cfg = parsed.get<Config>();
    normalize_native_only(cfg);
    return ConfigInitializationResult{ConfigInitializationStatus::Normal, cfg,
                                      "normal"};
  } catch (const std::exception &error) {
    exv::observability::LogFacade::error(
        "ensure_initialized_config: parse error: " + std::string(error.what()));
    return repaired_result(ConfigInitializationStatus::Invalid, "parse",
                           config_dir);
  }
}

nlohmann::json quick_start_request_data(
    const ConfigInitializationResult &result) {
  return json{{"reason", status_reason(result.status)},
              {"defaults",
               json{{"server", "vpn-ct.ecnu.edu.cn"},
                    {"remember_password", true},
                    {"install_service", true}}}};
}

} // namespace exv::config
```

- [ ] **Step 5: Add the new source to the core library**

In `CMakeLists.txt`, add `src/core/config/config_initialization.cpp` immediately after `src/core/config/config_manager.cpp` inside `EXV_CORE_SOURCES`:

```cmake
set(EXV_CORE_SOURCES
    src/core/config/config_manager.cpp
    src/core/config/config_initialization.cpp
    src/core/config/config_api.cpp
```

- [ ] **Step 6: Route `ConfigManager::load()` through the repair API**

In `src/core/config/config_manager.cpp`, add:

```cpp
#include "core/config/config_initialization.hpp"
```

Replace the body of `ConfigManager::load()` with:

```cpp
Config ConfigManager::load() {
    std::lock_guard<std::mutex> lock(mutex_);

    const auto initialized = ensure_initialized_config(config_dir_);
    config_ = initialized.config;
    normalize_native_only(config_);
    return config_;
}
```

- [ ] **Step 7: Route CLI config load through the same repair API**

In `src/core/config/config_persistence.cpp`, add:

```cpp
#include "core/config/config_initialization.hpp"
```

Replace the first half of `Config load()` with:

```cpp
Config load() {
  const auto initialized = ensure_initialized_config(platform::get_config_dir());
  Config cfg = initialized.config;
  normalize_native_only(cfg);
  if (initialized.status == ConfigInitializationStatus::Missing) {
    crypto::init_key_if_needed();
  }
  if (initialized.status == ConfigInitializationStatus::Invalid) {
    cli::print_warning("config.json was invalid or incomplete; defaults were regenerated.");
  }
  return cfg;
}
```

Keep `save`, `import_from`, and `reset` below it unchanged.

- [ ] **Step 8: Verify config initialization tests pass**

Run:

```powershell
cmake --build build --target config_initialization_test config_actions_test --config Debug
ctest --test-dir build -C Debug -R "^(config_initialization_test|config_actions_test)$" --output-on-failure
```

Expected: both tests pass. If `config_actions_test` now repairs legacy/incomplete fixtures, adjust its `write_legacy_config()` fixture to include all required fields or change that specific case to call `config.import` for legacy partial import behavior.

- [ ] **Step 9: Commit**

Run:

```powershell
git add CMakeLists.txt src/core/config/config_initialization.hpp src/core/config/config_initialization.cpp src/core/config/config_manager.cpp src/core/config/config_persistence.cpp tests/config_initialization_test.cpp tests/core_api/config_actions_test.cpp
git commit -m "core: repair incomplete startup config"
```

## Task 3: Emit and Preserve Core-Owned Quick-Start Requests

**Files:**

- Modify: `src/core/core_process.cpp`
- Modify: `tests/core_process_lifecycle_test.cpp`
- Modify: `src/app/ui_shell/core_rpc_client.hpp`
- Modify: `src/app/ui_shell/core_rpc_client.cpp`
- Modify: `src/app/ui_shell/ui_shell_runtime.cpp`

- [ ] **Step 1: Add core process lifecycle assertions**

In `tests/core_process_lifecycle_test.cpp`, add this helper after `find_by_request_id`:

```cpp
static json find_event(const std::vector<json>& responses,
                       const std::string& event_type) {
    for (auto& r : responses) {
        if (r.value("event", std::string()) == event_type) return r;
    }
    return json();
}
```

Replace `write_valid_native_config` with a complete initialized config:

```cpp
static void write_valid_native_config(const std::string& config_dir) {
    json cfg = {
        {"server", "https://vpn.example.invalid"},
        {"username", "student@example.invalid"},
        {"password", ""},
        {"mtu", 1290},
        {"useragent", "test-agent"},
        {"disable_dtls", false},
        {"remember_password", false},
        {"routes", json::array({"49.52.4.0/25"})},
        {"extra_args", json::array()},
        {"log_file", ""},
        {"vpn_engine", "native"},
        {"windows_tunnel_driver", "tap"},
        {"windows_tap_interface", "EXV TAP"},
        {"auto_reconnect", false},
        {"minimal_mode", false},
        {"service_install_prompt_seen", false},
        {"minimal_install_service_before_connect", true},
    };
    std::ofstream out(std::filesystem::path(config_dir) / "config.json",
                      std::ios::out | std::ios::trunc);
    out << cfg.dump(2);
}
```

Add this test block before `E2.1a`:

```cpp
    // =======================================================================
    // E2.0-quick-start - missing config emits quick-start-request once
    // =======================================================================
    {
        std::cerr << "[E2.0-quick-start] missing config emits quick start\n";
        auto quick_config_dir = make_temp_dir();
        auto quick_home_dir = make_temp_dir();

        BlockingInputBuf in_buf;
        CaptureOutputBuf out_buf;
        ScopedRdbuf sci(std::cin,  &in_buf);
        ScopedRdbuf sco(std::cout, &out_buf);
        std::cin.tie(nullptr);

        CoreRunner cr;
        cr.start(quick_config_dir, quick_home_dir);

        in_buf.feed(R"({"id":901,"action":"status.get","payload":{}})" "\n");
        expect(wait_for_response_count(out_buf, 1, std::chrono::seconds(3)),
               "E2.0-quick-start: should receive response");

        in_buf.close_input();
        cr.join();

        auto all = parse_json_lines(out_buf.read_all());
        auto event = find_event(all, "quick-start-request");
        expect(!event.is_null(),
               "E2.0-quick-start: missing config emits quick-start-request");
        if (!event.is_null()) {
            expect(event["data"].value("reason", std::string()) == "missing",
                   "E2.0-quick-start: reason is missing");
            expect(event["data"]["defaults"].value("server", std::string()) ==
                       "vpn-ct.ecnu.edu.cn",
                   "E2.0-quick-start: default server is ct host");
            expect(event["data"]["defaults"].value("remember_password", false),
                   "E2.0-quick-start: remember password defaults true");
            expect(event["data"]["defaults"].value("install_service", false),
                   "E2.0-quick-start: install service defaults true");
        }
        expect(std::filesystem::exists(std::filesystem::path(quick_config_dir) / "config.json"),
               "E2.0-quick-start: config file is generated before renderer actions");
        std::filesystem::remove_all(quick_config_dir);
        std::filesystem::remove_all(quick_home_dir);
    }
```

- [ ] **Step 2: Run the lifecycle test and verify failure**

Run:

```powershell
cmake --build build --target core_process_lifecycle_test --config Debug
ctest --test-dir build -C Debug -R "^core_process_lifecycle_test$" --output-on-failure
```

Expected: the new quick-start assertion fails because `core_process_main()` does not emit the event yet.

- [ ] **Step 3: Emit quick-start event from core startup**

In `src/core/core_process.cpp`, add:

```cpp
#include "core/config/config_initialization.hpp"
```

After logger setup and before handlers serve config actions, add:

```cpp
    const auto startup_config =
        exv::config::ensure_initialized_config(config_dir);
```

After the event pump is ready and before the main `while (!g_stop_requested.load())` loop, add:

```cpp
    if (stdin_available && startup_config.should_request_quick_start()) {
        write_json_line(json{{"event", "quick-start-request"},
                             {"data", exv::config::quick_start_request_data(
                                          startup_config)}});
    }
```

Keep daemon `use_stdin=false` silent; the desktop renderer only consumes stdin-mode events.

- [ ] **Step 4: Preserve quick-start events until the UI shell event handler exists**

In `src/app/ui_shell/core_rpc_client.hpp`, add a small backlog to `CoreRpcClient`:

```cpp
#include <deque>

private:
  std::deque<CoreRpcEvent> pending_events_;
```

In `src/app/ui_shell/core_rpc_client.cpp`, replace `set_event_handler` with:

```cpp
void CoreRpcClient::set_event_handler(CoreRpcEventHandler handler) {
  std::deque<CoreRpcEvent> pending;
  {
    std::lock_guard<std::mutex> lock(event_mutex_);
    event_handler_ = std::move(handler);
    if (event_handler_) {
      pending.swap(pending_events_);
    }
  }
  for (const auto &event : pending) {
    CoreRpcEventHandler current;
    {
      std::lock_guard<std::mutex> lock(event_mutex_);
      current = event_handler_;
    }
    if (current) {
      current(event);
    }
  }
}
```

In both places that currently call `handler(parse_core_rpc_event_line(...))`, replace that block with:

```cpp
        auto event = parse_core_rpc_event_line(line);
        CoreRpcEventHandler handler;
        {
          std::lock_guard<std::mutex> lock(event_mutex_);
          handler = event_handler_;
          if (!handler && event.event == "quick-start-request") {
            pending_events_.push_back(event);
          }
        }
        if (handler) {
          handler(event);
        }
```

Use `response_line` instead of `line` in the reader-loop copy:

```cpp
        auto event = parse_core_rpc_event_line(response_line);
```

- [ ] **Step 5: Verify core and UI shell tests**

Run:

```powershell
cmake --build build --target core_process_lifecycle_test ui_shell_runtime_test --config Debug
ctest --test-dir build -C Debug -R "^(core_process_lifecycle_test|ui_shell_runtime_test)$" --output-on-failure
```

Expected: both tests pass and no existing event transport test regresses.

- [ ] **Step 6: Commit**

Run:

```powershell
git add src/core/core_process.cpp tests/core_process_lifecycle_test.cpp src/app/ui_shell/core_rpc_client.hpp src/app/ui_shell/core_rpc_client.cpp src/app/ui_shell/ui_shell_runtime.cpp
git commit -m "core: emit quick start request from config repair"
```

## Task 4: Add Renderer State for Quick Start and Credential Completion

**Files:**

- Modify: `webui/src/types/exv.d.ts`
- Modify: `webui/src/stores/ui.ts`
- Modify: `webui/src/composables/useSSE.ts`
- Modify: `webui/host/__tests__/modal-onboarding-contract.test.ts`
- Modify: `webui/package.json`

- [ ] **Step 1: Create failing host source contract tests**

Create `webui/host/__tests__/modal-onboarding-contract.test.ts`:

```ts
import { describe, it } from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'

const webuiRoot = process.cwd()

function readSource(...parts: string[]) {
  return readFileSync(join(webuiRoot, ...parts), 'utf8')
}

describe('modal onboarding and credential contracts', () => {
  it('handles core-owned quick-start-request events in the renderer', () => {
    const exvTypes = readSource('src', 'types', 'exv.d.ts')
    const uiStore = readSource('src', 'stores', 'ui.ts')
    const useSse = readSource('src', 'composables', 'useSSE.ts')

    assert.match(exvTypes, /interface QuickStartRequestEvent/)
    assert.match(exvTypes, /reason:\s*'missing'\s*\|\s*'invalid'/)
    assert.match(uiStore, /showQuickStart/)
    assert.match(uiStore, /openQuickStart\(/)
    assert.match(useSse, /event\.type === 'quick-start-request'/)
    assert.match(useSse, /ui\.openQuickStart/)
  })

  it('exposes a typed credential prompt instead of password-only connect resolution', () => {
    const uiStore = readSource('src', 'stores', 'ui.ts')
    const vpnStore = readSource('src', 'stores', 'vpn.ts')

    assert.match(uiStore, /interface CredentialPromptRequest/)
    assert.match(uiStore, /requestCredentials\(/)
    assert.match(uiStore, /rememberPassword/)
    assert.match(vpnStore, /resolveConnectCredentials/)
    assert.doesNotMatch(vpnStore, /resolveConnectPassword\(/)
  })

  it('keeps frontend from owning service_install_prompt_seen', () => {
    const configStore = readSource('src', 'stores', 'config.ts')
    const app = readSource('src', 'App.vue')

    assert.doesNotMatch(configStore, /exv:service-install-prompt-seen/)
    assert.doesNotMatch(configStore, /delete remoteSettings\.service_install_prompt_seen/)
    assert.doesNotMatch(app, /markServicePromptSeen/)
    assert.doesNotMatch(app, /serviceInstallPrompt\(/)
  })
})
```

Update `webui/package.json` `test:host` so the new file runs:

```json
"test:host": "node scripts/run-host-test.cjs host/__tests__/host-boundary.test.ts host/__tests__/webview-package-policy.test.ts host/__tests__/desktop-contract-generated.test.ts host/__tests__/native-error-presentation.test.ts host/__tests__/ui-mode-and-connect-failure.test.ts host/__tests__/modal-onboarding-contract.test.ts"
```

- [ ] **Step 2: Run the failing host tests**

Run:

```powershell
pnpm --dir webui test:host
```

Expected: the new test fails because quick-start and credential prompt state are not implemented.

- [ ] **Step 3: Add typed quick-start event payload**

In `webui/src/types/exv.d.ts`, add:

```ts
export interface QuickStartRequestEvent {
  reason: 'missing' | 'invalid'
  defaults: {
    server: string
    remember_password: boolean
    install_service: boolean
  }
}
```

Change `ExvEvent` to:

```ts
export interface ExvEvent {
  type: DesktopEventType
  data: unknown | ServiceProgressEntry | QuickStartRequestEvent
}
```

- [ ] **Step 4: Add quick-start and credential prompt state to UI store**

In `webui/src/stores/ui.ts`, add these interfaces near `ErrorModalState`:

```ts
export interface QuickStartRequest {
  reason: 'missing' | 'invalid'
  defaults: {
    server: string
    remember_password: boolean
    install_service: boolean
  }
}

export interface CredentialPromptRequest {
  missingUsername: boolean
  missingPassword: boolean
  username: string
  rememberPassword: boolean
  message?: string
}

export interface CredentialPromptResult {
  username?: string
  password?: string
  rememberPassword: boolean
}
```

Inside the store, add state:

```ts
  const showQuickStart = ref(false)
  const quickStartRequest = ref<QuickStartRequest | null>(null)
  const showCredentialPrompt = ref(false)
  const credentialPrompt = ref<CredentialPromptRequest | null>(null)
  const credentialPromptResolver = ref<((value: CredentialPromptResult | null) => void) | null>(null)
```

Add actions:

```ts
  function openQuickStart(request: QuickStartRequest) {
    quickStartRequest.value = request
    showQuickStart.value = true
  }

  function closeQuickStart() {
    showQuickStart.value = false
    quickStartRequest.value = null
  }

  function requestCredentials(request: CredentialPromptRequest) {
    credentialPromptResolver.value?.(null)
    credentialPrompt.value = request
    showCredentialPrompt.value = true
    return new Promise<CredentialPromptResult | null>((resolve) => {
      credentialPromptResolver.value = resolve
    })
  }

  function submitCredentialPrompt(value: CredentialPromptResult) {
    const resolver = credentialPromptResolver.value
    showCredentialPrompt.value = false
    credentialPrompt.value = null
    credentialPromptResolver.value = null
    resolver?.(value)
  }

  function closeCredentialPrompt() {
    const resolver = credentialPromptResolver.value
    showCredentialPrompt.value = false
    credentialPrompt.value = null
    credentialPromptResolver.value = null
    resolver?.(null)
  }
```

Return the new state/actions from the store:

```ts
    showQuickStart, quickStartRequest,
    showCredentialPrompt, credentialPrompt,
    openQuickStart, closeQuickStart,
    requestCredentials, submitCredentialPrompt, closeCredentialPrompt,
```

- [ ] **Step 5: Wire SSE quick-start events to the UI store**

In `webui/src/composables/useSSE.ts`, import the UI store and type:

```ts
import { useUiStore } from '../stores/ui'
import type { QuickStartRequestEvent } from '../types/exv'
```

Add this block inside the subscription callback:

```ts
        if (event.type === 'quick-start-request' && event.data && typeof event.data === 'object') {
          const ui = useUiStore()
          ui.openQuickStart(event.data as QuickStartRequestEvent)
        }
```

- [ ] **Step 6: Verify host tests for this layer**

Run:

```powershell
pnpm --dir webui test:host
```

Expected: the new quick-start state assertions pass; later modal and VPN assertions still fail until following tasks.

- [ ] **Step 7: Commit**

Run:

```powershell
git add webui/src/types/exv.d.ts webui/src/stores/ui.ts webui/src/composables/useSSE.ts webui/host/__tests__/modal-onboarding-contract.test.ts webui/package.json
git commit -m "webui: add quick start and credential prompt state"
```

## Task 5: Replace Frontend-Owned Service Prompt State

**Files:**

- Modify: `webui/src/stores/config.ts`
- Modify: `webui/src/App.vue`
- Modify: `webui/host/__tests__/ui-mode-and-connect-failure.test.ts`

- [ ] **Step 1: Update old localStorage assertions to the new ownership model**

In `webui/host/__tests__/ui-mode-and-connect-failure.test.ts`, replace the first two tests in `describe('frontend-owned UI mode state'...)` with:

```ts
describe('frontend-owned UI mode state', () => {
  it('keeps only minimal mode in renderer localStorage', () => {
    const literals = stringLiterals(configStoreText)
    assert.ok(literals.includes('exv:minimal-mode'))
    assert.equal(literals.includes('exv:service-install-prompt-seen'), false)
    assert.ok(hasPropertyCall(configStoreText, 'getItem'))
    assert.ok(hasPropertyCall(configStoreText, 'setItem'))

    const names = functionNames(configStoreText)
    assert.ok(names.has('applyFrontendLocalSettings'))
    assert.ok(names.has('persistFrontendLocalSettings'))
  })

  it('sends service prompt config fields through core-owned settings unchanged', () => {
    const deleted = deletePropertyNames(configStoreText)
    assert.ok(deleted.has('minimal_mode'))
    assert.equal(deleted.has('service_install_prompt_seen'), false)
    assert.ok(hasObjectKeysLengthZeroReturn(configStoreText, 'remoteSettings'))
  })
```

- [ ] **Step 2: Run failing host tests**

Run:

```powershell
pnpm --dir webui test:host
```

Expected: tests fail because `config.ts` still reads and writes `exv:service-install-prompt-seen`, and `App.vue` still opens the service prompt on mount.

- [ ] **Step 3: Remove frontend service prompt localStorage overlay**

In `webui/src/stores/config.ts`, change helper signatures:

```ts
  function readLocalBool(key: 'exv:minimal-mode', fallback: boolean) {
    if (typeof localStorage === 'undefined') return fallback
    const value = localStorage.getItem('exv:minimal-mode')
    if (value === 'true') return true
    if (value === 'false') return false
    return fallback
  }

  function writeLocalBool(key: 'exv:minimal-mode', value: boolean) {
    if (typeof localStorage === 'undefined') return
    localStorage.setItem('exv:minimal-mode', value ? 'true' : 'false')
  }
```

Change `applyFrontendLocalSettings` to:

```ts
  function applyFrontendLocalSettings(next: SettingsConfig) {
    return {
      ...next,
      minimal_mode: readLocalBool('exv:minimal-mode', next.minimal_mode),
    }
  }
```

Change `persistFrontendLocalSettings` to:

```ts
  function persistFrontendLocalSettings(s: Partial<SettingsConfig>) {
    if (Object.prototype.hasOwnProperty.call(s, 'minimal_mode') && s.minimal_mode != null) {
      writeLocalBool('exv:minimal-mode', s.minimal_mode)
    }
  }
```

In `saveSettings`, remove this line:

```ts
    delete remoteSettings.service_install_prompt_seen
```

Keep `service_install_prompt_seen` in the `SettingsConfig` interface and default state because core still persists it for compatibility and completeness.

- [ ] **Step 4: Remove service-install prompt ownership from App**

In `webui/src/App.vue`, remove:

```ts
const servicePromptVisible = ref(false)
const servicePromptBusy = ref(false)
```

Remove the `onMounted` block that checks:

```ts
  if (!config.settings.service_install_prompt_seen && !vpn.serviceInstalled) {
```

Remove the `markServicePromptSeen()` function.

Remove any `<ServiceInstallPromptModal>` usage if still present. Keep `ServiceInstallLoadingOverlay`; quick start and dashboard installation still use `vpn.installService()`.

- [ ] **Step 5: Verify host tests pass for ownership**

Run:

```powershell
pnpm --dir webui test:host
```

Expected: service prompt ownership tests pass. Modal and credential tests may still fail until later tasks if they are now included.

- [ ] **Step 6: Commit**

Run:

```powershell
git add webui/src/stores/config.ts webui/src/App.vue webui/host/__tests__/ui-mode-and-connect-failure.test.ts
git commit -m "webui: remove renderer-owned first run service prompt"
```

## Task 6: Add Shared Compact Modal Foundation and Clip Scrims to the Window Surface

**Files:**

- Create: `webui/src/components/ModalShell.vue`
- Modify: `webui/src/App.vue`
- Modify: `webui/src/components/ErrorDialog.vue`
- Modify: `webui/src/components/ConfirmDialog.vue`
- Modify: `webui/src/components/AuthContinuationDialog.vue`
- Modify: `webui/src/components/CoreCrashed.vue`
- Modify: `webui/src/components/ServiceInstallLoadingOverlay.vue`
- Modify: `webui/src/pages/ServiceInstallPromptModal.vue`
- Modify: `webui/src/style.css`
- Modify: `webui/host/__tests__/modal-onboarding-contract.test.ts`

- [ ] **Step 1: Add failing modal shell source assertions**

Extend `webui/host/__tests__/modal-onboarding-contract.test.ts` with:

```ts
  it('mounts all in-window modals through the shared clipped modal shell', () => {
    const app = readSource('src', 'App.vue')
    const modalShell = readSource('src', 'components', 'ModalShell.vue')
    const modalConsumers = [
      'ErrorDialog.vue',
      'ConfirmDialog.vue',
      'AuthContinuationDialog.vue',
      'CoreCrashed.vue',
      'ServiceInstallLoadingOverlay.vue',
    ]

    assert.match(modalShell, /class="modal-shell__scrim"/)
    assert.match(modalShell, /position:\s*absolute/)
    assert.match(modalShell, /inset:\s*0/)
    assert.doesNotMatch(modalShell, /Teleport/)
    assert.match(app, /<ErrorDialog \/>[\s\S]*<\/AppWindowFrame>/)

    for (const file of modalConsumers) {
      const source = readSource('src', 'components', file)
      assert.match(source, /ModalShell/)
      assert.doesNotMatch(source, /fixed inset-0/)
      assert.doesNotMatch(source, /Teleport/)
    }
  })
```

- [ ] **Step 2: Run failing modal source tests**

Run:

```powershell
pnpm --dir webui test:host
```

Expected: the modal shell file does not exist and modal consumers still use fixed full-viewport overlays.

- [ ] **Step 3: Create `ModalShell.vue`**

Create `webui/src/components/ModalShell.vue`:

```vue
<script setup lang="ts">
const props = withDefaults(defineProps<{
  open: boolean
  title?: string
  description?: string
  size?: 'sm' | 'md' | 'lg'
  closeOnScrim?: boolean
}>(), {
  title: '',
  description: '',
  size: 'md',
  closeOnScrim: true,
})

const emit = defineEmits<{
  close: []
}>()

function onScrimClick() {
  if (props.closeOnScrim) emit('close')
}
</script>

<template>
  <div
    v-if="open"
    class="modal-shell__scrim"
    role="presentation"
    @click.self="onScrimClick"
  >
    <section
      class="modal-shell__panel"
      :class="`modal-shell__panel--${size}`"
      role="dialog"
      aria-modal="true"
      :aria-label="title || undefined"
    >
      <header v-if="title || description || $slots.icon" class="modal-shell__header">
        <div v-if="$slots.icon" class="modal-shell__icon">
          <slot name="icon" />
        </div>
        <div class="modal-shell__heading">
          <h2 v-if="title" class="modal-shell__title">{{ title }}</h2>
          <p v-if="description" class="modal-shell__description">{{ description }}</p>
        </div>
      </header>

      <div class="modal-shell__body">
        <slot />
      </div>

      <footer v-if="$slots.actions" class="modal-shell__actions">
        <slot name="actions" />
      </footer>
    </section>
  </div>
</template>
```

Add modal CSS to `webui/src/style.css`:

```css
.modal-shell__scrim {
  position: absolute;
  inset: 0;
  z-index: 120;
  display: grid;
  place-items: center;
  padding: 14px;
  background: rgba(2, 6, 23, 0.56);
  backdrop-filter: blur(10px) saturate(1.05);
}

.modal-shell__panel {
  width: min(100%, 400px);
  max-height: min(82vh, 520px);
  overflow: auto;
  border: 1px solid rgba(148, 163, 184, 0.24);
  border-radius: 8px;
  background: #111827;
  color: #f8fafc;
  box-shadow: 0 18px 40px rgba(0, 0, 0, 0.36);
  padding: 18px;
}

.modal-shell__panel--sm {
  width: min(100%, 360px);
}

.modal-shell__panel--lg {
  width: min(100%, 440px);
}

.modal-shell__header {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  margin-bottom: 14px;
}

.modal-shell__icon {
  display: grid;
  width: 28px;
  height: 28px;
  flex: 0 0 auto;
  place-items: center;
  border-radius: 8px;
  background: rgba(34, 197, 94, 0.12);
  color: #22c55e;
}

.modal-shell__heading {
  min-width: 0;
}

.modal-shell__title {
  margin: 0;
  font-size: 15px;
  font-weight: 650;
  line-height: 1.35;
  letter-spacing: 0;
}

.modal-shell__description {
  margin: 4px 0 0;
  color: #94a3b8;
  font-size: 12px;
  line-height: 1.45;
}

.modal-shell__body {
  min-width: 0;
}

.modal-shell__actions {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}
```

- [ ] **Step 4: Mount modal consumers inside `AppWindowFrame`**

In `webui/src/App.vue`, move these components so they are the final children inside `<AppWindowFrame>`:

```vue
    <ServiceInstallLoadingOverlay
      v-if="vpn.serviceOverlayOperation"
      :message="serviceOverlayMessage"
    />
    <CoreCrashed
      v-if="coreCrashed"
      :exit-code="coreCrashInfo?.exitCode ?? null"
      @restart="handleCoreRestart"
      @quit="handleCoreQuit"
    />
    <ErrorDialog />
    <ConfirmDialog />
    <AuthContinuationDialog />
    <CredentialPromptDialog />
    <QuickStartDialog />
    <ToastStack />
```

Remove the old sibling copies after `</AppWindowFrame>`. This makes the `ModalShell` absolute scrim use `.mode-transition-surface` as its containing block and inherit clipping from `overflow: hidden`.

- [ ] **Step 5: Convert simple modal consumers to `ModalShell`**

For each converted component, import `ModalShell`:

```ts
import ModalShell from './ModalShell.vue'
```

Use this template shape for `ErrorDialog.vue`:

```vue
<ModalShell
  :open="ui.errorModal.visible"
  :title="ui.errorModal.title"
  :description="ui.errorModal.message"
  size="sm"
  @close="ui.closeError"
>
  <template #actions>
    <button type="button" class="rounded-lg border border-border px-3 py-2 text-sm text-muted hover:bg-surface/80" @click="ui.closeError">
      {{ ui.errorModal.secondaryLabel }}
    </button>
    <button type="button" class="rounded-lg bg-primary px-3 py-2 text-sm text-white hover:bg-primary/90" @click="ui.onErrorPrimary">
      {{ ui.errorModal.primaryLabel }}
    </button>
  </template>
</ModalShell>
```

Use the same shell pattern for:

- `ConfirmDialog.vue`: `open=ui.showConfirm`, title `确认操作`, description `ui.confirmMessage`, actions cancel/confirm.
- `AuthContinuationDialog.vue`: preserve its existing field body and action handlers inside the default/action slots.
- `CoreCrashed.vue`: title `核心进程已退出`, no scrim close, restart/quit actions.
- `ServiceInstallLoadingOverlay.vue`: no scrim close, compact progress body, no nested card.

- [ ] **Step 6: Compact the native service modal route**

In `webui/src/pages/ServiceInstallPromptModal.vue`, keep the native route behavior but align tokens:

```vue
<main class="min-h-screen bg-transparent p-3 text-foreground">
  <section class="mx-auto w-full max-w-sm rounded-lg border border-border bg-surface p-4 shadow-xl">
```

Use 14 to 16 px body text and 36 px minimum button height. Do not add a full-viewport dark scrim on the native modal route.

- [ ] **Step 7: Verify modal tests**

Run:

```powershell
pnpm --dir webui test:host
pnpm --dir webui exec vue-tsc -b
```

Expected: static modal shell tests pass and TypeScript reports no component import or prop errors.

- [ ] **Step 8: Commit**

Run:

```powershell
git add webui/src/components/ModalShell.vue webui/src/App.vue webui/src/components/ErrorDialog.vue webui/src/components/ConfirmDialog.vue webui/src/components/AuthContinuationDialog.vue webui/src/components/CoreCrashed.vue webui/src/components/ServiceInstallLoadingOverlay.vue webui/src/pages/ServiceInstallPromptModal.vue webui/src/style.css webui/host/__tests__/modal-onboarding-contract.test.ts
git commit -m "webui: unify compact clipped modal shell"
```

## Task 7: Implement Credential Completion for Connect

**Files:**

- Create: `webui/src/components/CredentialPromptDialog.vue`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/App.vue`
- Modify: `webui/src/components/PasswordPromptDialog.vue`
- Modify: `webui/host/__tests__/modal-onboarding-contract.test.ts`

- [ ] **Step 1: Add credential behavior source assertions**

Extend `modal-onboarding-contract.test.ts` with:

```ts
  it('credential prompt supports missing username, missing password, and remembered password saves', () => {
    const dialog = readSource('src', 'components', 'CredentialPromptDialog.vue')
    const vpnStore = readSource('src', 'stores', 'vpn.ts')

    assert.match(dialog, /missingUsername/)
    assert.match(dialog, /missingPassword/)
    assert.match(dialog, /rememberPassword/)
    assert.match(dialog, /ui\.submitCredentialPrompt/)
    assert.match(vpnStore, /const missingUsername = !auth\.username\.trim\(\)/)
    assert.match(vpnStore, /const missingPassword = !\(auth\.remember_password && auth\.password_stored\)/)
    assert.match(vpnStore, /remember_password: credentials\.rememberPassword/)
    assert.match(vpnStore, /password: credentials\.rememberPassword \? credentials\.password/)
  })
```

- [ ] **Step 2: Run failing tests**

Run:

```powershell
pnpm --dir webui test:host
```

Expected: the new dialog file is missing and `vpn.ts` still uses `resolveConnectPassword`.

- [ ] **Step 3: Add `CredentialPromptDialog.vue`**

Create `webui/src/components/CredentialPromptDialog.vue`:

```vue
<script setup lang="ts">
import { nextTick, ref, watch } from 'vue'
import { Eye, KeyRound } from 'lucide-vue-next'
import ModalShell from './ModalShell.vue'
import { useUiStore } from '../stores/ui'

const ui = useUiStore()
const username = ref('')
const password = ref('')
const rememberPassword = ref(true)
const error = ref('')
const revealing = ref(false)
const usernameRef = ref<HTMLInputElement | null>(null)
const passwordRef = ref<HTMLInputElement | null>(null)

watch(
  () => ui.showCredentialPrompt,
  async (visible) => {
    if (!visible || !ui.credentialPrompt) {
      username.value = ''
      password.value = ''
      error.value = ''
      revealing.value = false
      return
    }
    username.value = ui.credentialPrompt.username
    password.value = ''
    rememberPassword.value = ui.credentialPrompt.rememberPassword
    error.value = ''
    revealing.value = false
    await nextTick()
    if (ui.credentialPrompt.missingUsername) {
      usernameRef.value?.focus()
    } else {
      passwordRef.value?.focus()
    }
  },
)

function submit() {
  const request = ui.credentialPrompt
  if (!request) return
  if (request.missingUsername && !username.value.trim()) {
    error.value = '请填写用户名'
    usernameRef.value?.focus()
    return
  }
  if (request.missingPassword && !password.value) {
    error.value = '请填写密码'
    passwordRef.value?.focus()
    return
  }
  ui.submitCredentialPrompt({
    username: request.missingUsername ? username.value.trim() : undefined,
    password: request.missingPassword ? password.value : undefined,
    rememberPassword: rememberPassword.value,
  })
}

function cancel() {
  ui.closeCredentialPrompt()
}
</script>

<template>
  <ModalShell
    :open="ui.showCredentialPrompt"
    title="补全连接凭据"
    :description="ui.credentialPrompt?.message || '连接前需要补全缺少的认证信息。'"
    size="sm"
    @close="cancel"
  >
    <template #icon>
      <KeyRound class="h-4 w-4" />
    </template>

    <form class="space-y-3" @submit.prevent="submit">
      <label v-if="ui.credentialPrompt?.missingUsername" class="block">
        <span class="mb-1 block text-xs font-medium text-muted">用户名</span>
        <input
          ref="usernameRef"
          v-model="username"
          autocomplete="username"
          class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground outline-none focus:border-primary"
          @input="error = ''"
          @keydown.esc.prevent="cancel"
        />
      </label>

      <label v-if="ui.credentialPrompt?.missingPassword" class="block">
        <span class="mb-1 block text-xs font-medium text-muted">密码</span>
        <span class="relative block">
          <input
            ref="passwordRef"
            v-model="password"
            :type="revealing ? 'text' : 'password'"
            autocomplete="current-password"
            class="w-full rounded-lg border border-border bg-bg px-3 py-2 pr-11 text-sm text-foreground outline-none focus:border-primary"
            @input="error = ''"
            @keydown.esc.prevent="cancel"
          />
          <button
            type="button"
            class="absolute right-2 top-1/2 grid h-8 w-8 -translate-y-1/2 place-items-center rounded-md text-muted hover:bg-surface/80 hover:text-foreground"
            title="按住显示密码"
            aria-label="按住显示密码"
            @pointerdown.prevent="revealing = true"
            @pointerup="revealing = false"
            @pointercancel="revealing = false"
            @pointerleave="revealing = false"
            @blur="revealing = false"
          >
            <Eye class="h-4 w-4" />
          </button>
        </span>
      </label>

      <label v-if="ui.credentialPrompt?.missingPassword" class="flex items-center gap-2 text-xs text-muted">
        <input
          v-model="rememberPassword"
          type="checkbox"
          class="h-4 w-4 rounded border-border bg-bg text-primary focus:ring-primary/40"
        />
        记住密码
      </label>

      <p v-if="error" class="text-xs text-destructive">{{ error }}</p>

      <div class="flex justify-end gap-2 pt-1">
        <button type="button" class="rounded-lg border border-border px-3 py-2 text-sm text-muted hover:bg-surface/80" @click="cancel">
          取消
        </button>
        <button type="submit" class="rounded-lg bg-primary px-3 py-2 text-sm text-white hover:bg-primary/90">
          连接
        </button>
      </div>
    </form>
  </ModalShell>
</template>
```

- [ ] **Step 4: Replace password-only connect resolution**

In `webui/src/stores/vpn.ts`, remove `buildPasswordPromptMessage` and `resolveConnectPassword`. Add:

```ts
  function buildCredentialPromptMessage(prefix = '') {
    const auth = config.authConfig
    const username = auth.username || status.value?.username
    const base = username ? `请补全 ${username} 的连接凭据` : '请补全 VPN 连接凭据'
    return prefix ? `${prefix}${base}` : base
  }

  async function resolveConnectCredentials(messagePrefix = ''): Promise<{ password?: string } | null> {
    await config.fetchAuthConfig()
    const auth = config.authConfig
    const missingUsername = !auth.username.trim()
    const missingPassword = !(auth.remember_password && auth.password_stored)

    if (!missingUsername && !missingPassword) return {}

    const credentials = await ui.requestCredentials({
      missingUsername,
      missingPassword,
      username: auth.username,
      rememberPassword: auth.remember_password,
      message: buildCredentialPromptMessage(messagePrefix),
    })
    if (credentials === null) return null

    const nextUsername = credentials.username ?? auth.username
    const nextPassword = credentials.password ?? ''

    await config.saveAuthConfig({
      server: auth.server,
      username: nextUsername,
      user_agent: auth.user_agent,
      remember_password: credentials.rememberPassword,
      password: credentials.rememberPassword ? nextPassword : '',
    })

    if (missingPassword && !credentials.rememberPassword) {
      return { password: nextPassword }
    }
    return {}
  }
```

In `connect`, replace password resolution with:

```ts
      const credentials = providedPassword !== undefined
        ? { password: providedPassword }
        : await resolveConnectCredentials()
      if (credentials === null) return false

      const { data } = await api.post<VpnStatus | VpnConnectAccepted>(
        '/connect',
        credentials.password === undefined ? undefined : { password: credentials.password },
      )
```

In `connectElevated`, use the same pattern for `/connect/elevated`.

In `retryConnectAfterAuthFailure`, replace `ui.requestPassword(...)` with:

```ts
    const credentials = await resolveConnectCredentials('密码不正确，请重新输入。')
    if (credentials === null) return false
    clearError()
    const password = credentials.password
    return mode === 'helper' ? connect(password) : connectElevated(password)
```

- [ ] **Step 5: Keep legacy `PasswordPromptDialog` only for import/native modal prompts**

Convert `PasswordPromptDialog.vue` to use `ModalShell`, but keep `ui.requestPassword` unchanged for protected config import and native modal compatibility. The password prompt must no longer be used by connect flows after Step 4.

- [ ] **Step 6: Mount the credential dialog**

In `webui/src/App.vue`, import:

```ts
import CredentialPromptDialog from './components/CredentialPromptDialog.vue'
```

Mount it inside `AppWindowFrame`:

```vue
    <CredentialPromptDialog />
```

- [ ] **Step 7: Verify**

Run:

```powershell
pnpm --dir webui test:host
pnpm --dir webui exec vue-tsc -b
```

Expected: host credential assertions pass and TypeScript reports no errors.

- [ ] **Step 8: Commit**

Run:

```powershell
git add webui/src/components/CredentialPromptDialog.vue webui/src/stores/vpn.ts webui/src/App.vue webui/src/components/PasswordPromptDialog.vue webui/host/__tests__/modal-onboarding-contract.test.ts
git commit -m "webui: prompt for missing connect credentials"
```

## Task 8: Implement Quick Start Dialog

**Files:**

- Create: `webui/src/components/QuickStartDialog.vue`
- Modify: `webui/src/App.vue`
- Modify: `webui/host/__tests__/modal-onboarding-contract.test.ts`

- [ ] **Step 1: Add quick start source assertions**

Extend `modal-onboarding-contract.test.ts` with:

```ts
  it('quick start supports quick/custom modes, import, skip, and service install default', () => {
    const dialog = readSource('src', 'components', 'QuickStartDialog.vue')
    const app = readSource('src', 'App.vue')

    assert.match(dialog, /mode = ref<'quick' \| 'custom'>\('quick'\)/)
    assert.match(dialog, /vpn-ct\.ecnu\.edu\.cn/)
    assert.match(dialog, /remember_password:\s*true/)
    assert.match(dialog, /installService/)
    assert.match(dialog, /TokenInput/)
    assert.match(dialog, /settingsForm\.mtu/)
    assert.match(dialog, /settingsForm\.dtls/)
    assert.match(dialog, /importConfig/)
    assert.match(dialog, /skip\(\)/)
    assert.match(app, /<QuickStartDialog \/>/)
  })
```

- [ ] **Step 2: Run failing tests**

Run:

```powershell
pnpm --dir webui test:host
```

Expected: `QuickStartDialog.vue` does not exist.

- [ ] **Step 3: Create `QuickStartDialog.vue`**

Create `webui/src/components/QuickStartDialog.vue` with this structure:

```vue
<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { FileUp, Rocket, Settings2 } from 'lucide-vue-next'
import ModalShell from './ModalShell.vue'
import TokenInput from './TokenInput.vue'
import { useConfigStore } from '../stores/config'
import { normalizeError, useVpnStore } from '../stores/vpn'
import { useUiStore } from '../stores/ui'

const ui = useUiStore()
const config = useConfigStore()
const vpn = useVpnStore()

const mode = ref<'quick' | 'custom'>('quick')
const username = ref('')
const password = ref('')
const routes = ref<string[]>([])
const settingsForm = ref({
  mtu: 1290,
  dtls: true,
})
const busy = ref(false)
const error = ref('')
const fileInput = ref<HTMLInputElement | null>(null)

const panelSize = computed(() => mode.value === 'custom' ? 'lg' : 'md')

watch(
  () => ui.showQuickStart,
  async (visible) => {
    if (!visible) return
    mode.value = 'quick'
    error.value = ''
    username.value = ''
    password.value = ''
    routes.value = vpn.routes.map((route) => route.cidr)
    settingsForm.value = {
      mtu: config.settings.mtu || 1290,
      dtls: config.settings.dtls,
    }
    await Promise.allSettled([
      config.fetchAuthConfig(),
      config.fetchSettings(),
      vpn.fetchRoutes(),
      vpn.fetchServiceStatus(),
    ])
    username.value = config.authConfig.username || ''
    routes.value = vpn.routes.map((route) => route.cidr)
    settingsForm.value = {
      mtu: config.settings.mtu || 1290,
      dtls: config.settings.dtls,
    }
  },
)

function skip() {
  ui.closeQuickStart()
}

function validate() {
  if (!username.value.trim()) {
    error.value = '请填写用户名'
    return false
  }
  if (!password.value) {
    error.value = '请填写密码'
    return false
  }
  if (mode.value === 'custom' && (settingsForm.value.mtu < 576 || settingsForm.value.mtu > 1500)) {
    error.value = 'MTU 必须在 576 到 1500 之间'
    return false
  }
  error.value = ''
  return true
}

async function saveCustomSettings() {
  await config.saveSettings({
    mtu: settingsForm.value.mtu,
    dtls: settingsForm.value.dtls,
  })
  await vpn.resetRoutes()
  for (const route of routes.value.map((item) => item.trim()).filter(Boolean)) {
    await vpn.addRoute(route)
  }
}

async function confirm() {
  if (!validate() || busy.value) return
  busy.value = true
  try {
    await config.saveAuthConfig({
      server: 'vpn-ct.ecnu.edu.cn',
      username: username.value.trim(),
      password: password.value,
      remember_password: true,
      user_agent: config.authConfig.user_agent,
    })
    if (mode.value === 'custom') {
      await saveCustomSettings()
    }
    ui.closeQuickStart()
    if (!vpn.serviceInstalled) {
      const installed = await vpn.installService()
      if (!installed) {
        ui.requestError({ title: '服务安装失败', message: '配置已保存，可稍后在设置中重新安装服务。' })
      }
    }
  } catch (err) {
    error.value = normalizeError(err).message
  } finally {
    busy.value = false
  }
}

function openImport() {
  fileInput.value?.click()
}

async function onImportFile(event: Event) {
  const target = event.target as HTMLInputElement
  const file = target.files?.[0]
  target.value = ''
  if (!file) return
  busy.value = true
  try {
    const text = await file.text()
    const parsed = JSON.parse(text)
    const format = parsed.format === 'protected' ? 'protected' : 'unprotected'
    let importPassword: string | undefined
    if (format === 'protected') {
      const passwordValue = await ui.requestPassword('请输入导入配置的保护密码')
      if (passwordValue === null) return
      importPassword = passwordValue
    }
    await config.importConfig({ format, data: text, password: importPassword })
    await Promise.all([config.fetchAuthConfig(), config.fetchSettings(), vpn.fetchRoutes()])
    ui.closeQuickStart()
  } catch (err) {
    error.value = normalizeError(err).message
  } finally {
    busy.value = false
  }
}
</script>

<template>
  <ModalShell
    :open="ui.showQuickStart"
    title="快速入门"
    :description="ui.quickStartRequest?.reason === 'invalid' ? '配置文件不完整，已重新初始化。' : '首次使用前补全连接信息。'"
    :size="panelSize"
    @close="skip"
  >
    <template #icon>
      <Rocket class="h-4 w-4" />
    </template>

    <input ref="fileInput" type="file" class="hidden" accept="application/json,.json" @change="onImportFile" />

    <div class="mb-3 grid grid-cols-2 rounded-lg border border-border bg-bg p-1 text-sm">
      <button
        type="button"
        class="rounded-md px-3 py-2"
        :class="mode === 'quick' ? 'bg-surface text-foreground' : 'text-muted hover:text-foreground'"
        @click="mode = 'quick'"
      >
        快速
      </button>
      <button
        type="button"
        class="rounded-md px-3 py-2"
        :class="mode === 'custom' ? 'bg-surface text-foreground' : 'text-muted hover:text-foreground'"
        @click="mode = 'custom'"
      >
        自定义
      </button>
    </div>

    <div class="space-y-3">
      <label class="block">
        <span class="mb-1 block text-xs font-medium text-muted">用户名</span>
        <input v-model="username" class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground outline-none focus:border-primary" autocomplete="username" />
      </label>
      <label class="block">
        <span class="mb-1 block text-xs font-medium text-muted">密码</span>
        <input v-model="password" class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground outline-none focus:border-primary" type="password" autocomplete="current-password" />
      </label>

      <div v-if="mode === 'custom'" class="space-y-3">
        <label class="block">
          <span class="mb-1 block text-xs font-medium text-muted">路由</span>
          <TokenInput v-model="routes" placeholder="输入 CIDR 后回车" />
        </label>
        <label class="flex items-center justify-between gap-3 rounded-lg border border-border bg-bg px-3 py-2 text-sm">
          <span class="flex items-center gap-2 text-foreground"><Settings2 class="h-4 w-4" /> DTLS</span>
          <input v-model="settingsForm.dtls" type="checkbox" class="h-4 w-4 rounded border-border bg-bg text-primary" />
        </label>
        <label class="block">
          <span class="mb-1 block text-xs font-medium text-muted">MTU</span>
          <input v-model.number="settingsForm.mtu" min="576" max="1500" type="number" class="w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground outline-none focus:border-primary" />
        </label>
      </div>
    </div>

    <p v-if="error" class="mt-3 text-xs text-destructive">{{ error }}</p>

    <template #actions>
      <button type="button" class="mr-auto inline-flex items-center gap-2 rounded-lg border border-border px-3 py-2 text-sm text-muted hover:bg-surface/80" :disabled="busy" @click="openImport">
        <FileUp class="h-4 w-4" />
        导入配置
      </button>
      <button type="button" class="rounded-lg border border-border px-3 py-2 text-sm text-muted hover:bg-surface/80" :disabled="busy" @click="skip">
        跳过
      </button>
      <button type="button" class="rounded-lg bg-primary px-3 py-2 text-sm text-white hover:bg-primary/90 disabled:opacity-60" :disabled="busy" @click="confirm">
        {{ busy ? '处理中...' : '确认' }}
      </button>
    </template>
  </ModalShell>
</template>
```

- [ ] **Step 4: Mount quick start in `App.vue`**

Import:

```ts
import QuickStartDialog from './components/QuickStartDialog.vue'
```

Mount inside `AppWindowFrame`:

```vue
    <QuickStartDialog />
```

- [ ] **Step 5: Verify quick start tests and types**

Run:

```powershell
pnpm --dir webui test:host
pnpm --dir webui exec vue-tsc -b
```

Expected: quick start static assertions pass and TypeScript reports no errors.

- [ ] **Step 6: Commit**

Run:

```powershell
git add webui/src/components/QuickStartDialog.vue webui/src/App.vue webui/host/__tests__/modal-onboarding-contract.test.ts
git commit -m "webui: add core-owned quick start dialog"
```

## Task 9: Fix Settings Select Readability

**Files:**

- Modify: `webui/src/pages/SettingsPage.vue`
- Modify: `webui/src/style.css`
- Modify: `webui/host/__tests__/modal-onboarding-contract.test.ts`

- [ ] **Step 1: Add failing select readability assertions**

Extend `modal-onboarding-contract.test.ts` with:

```ts
  it('styles native select options with readable dark theme colors', () => {
    const settings = readSource('src', 'pages', 'SettingsPage.vue')
    const css = readSource('src', 'style.css')

    assert.match(settings, /class="[^"]*exv-select/)
    assert.match(css, /\.exv-select option/)
    assert.match(css, /background:\s*#0f172a/)
    assert.match(css, /color:\s*#f8fafc/)
  })
```

- [ ] **Step 2: Run failing host tests**

Run:

```powershell
pnpm --dir webui test:host
```

Expected: select assertions fail because the class and option CSS are missing.

- [ ] **Step 3: Add the select class to native selects**

In `webui/src/pages/SettingsPage.vue`, append `exv-select` to each native select class:

```vue
class="exv-select w-full appearance-none rounded-lg border border-border bg-background py-2.5 pl-10 pr-4 text-sm text-foreground focus:border-accent focus:outline-none"
```

```vue
class="exv-select w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground transition-colors focus:border-accent/50 focus:outline-none"
```

```vue
class="exv-select w-full rounded-lg border border-border bg-bg px-3 py-2 text-sm text-foreground transition-colors focus:border-accent/50 focus:outline-none disabled:opacity-50"
```

- [ ] **Step 4: Add readable option CSS**

In `webui/src/style.css`, add:

```css
.exv-select {
  color-scheme: dark;
}

.exv-select option {
  background: #0f172a;
  color: #f8fafc;
}

.exv-select option:checked {
  background: #1e293b;
  color: #ffffff;
}
```

- [ ] **Step 5: Verify**

Run:

```powershell
pnpm --dir webui test:host
pnpm --dir webui exec vue-tsc -b
```

Expected: select readability assertions pass.

- [ ] **Step 6: Commit**

Run:

```powershell
git add webui/src/pages/SettingsPage.vue webui/src/style.css webui/host/__tests__/modal-onboarding-contract.test.ts
git commit -m "webui: improve dark select readability"
```

## Task 10: Final Verification and Visual QA

**Files:**

- Read/verify only unless a failure requires a focused fix.

- [ ] **Step 1: Run the complete host and type checks**

Run:

```powershell
pnpm --dir webui test:host
pnpm --dir webui exec vue-tsc -b
```

Expected: both commands pass.

- [ ] **Step 2: Run focused C++ tests**

Run:

```powershell
cmake --build build --target config_initialization_test config_actions_test contract_manifest_test core_process_lifecycle_test ui_shell_runtime_test --config Debug
ctest --test-dir build -C Debug -R "^(config_initialization_test|config_actions_test|contract_manifest_test|core_process_lifecycle_test|ui_shell_runtime_test)$" --output-on-failure
```

Expected: all listed C++ tests pass.

- [ ] **Step 3: Run the contract generator check**

Run:

```powershell
python scripts/generate_contracts.py --check
```

Expected: no generated contract drift.

- [ ] **Step 4: Build the WebUI**

Run:

```powershell
pnpm --dir webui build
```

Expected: Vue type check and Vite build pass.

- [ ] **Step 5: Run manual browser visual verification**

Start the renderer:

```powershell
pnpm --dir webui dev:desktop
```

Open the local Vite URL shown by the command, then verify:

- Modal scrim starts inside the visible thin border and does not tint the transparent shadow gutter.
- Error, confirm, credential, quick-start, core-crashed, and service loading modals use compact spacing.
- Quick mode shows username/password only.
- Custom mode adds routes, DTLS, and MTU.
- Import config button is visible in both modes.
- Skip closes quick start without saving or installing service.
- Settings select dropdown options have readable dark foreground/background.

Stop the dev server after verification.

- [ ] **Step 6: Inspect git diff**

Run:

```powershell
git status --short
git diff --stat
git diff --check
```

Expected: only files from this plan are modified, and `git diff --check` reports no whitespace errors.

- [ ] **Step 7: Final commit if fixes were made after Task 9**

Run only if Step 1 through Step 6 required additional fixes:

```powershell
git add .
git commit -m "test: verify modal onboarding config flow"
```

## Self-Review

Spec coverage:

- Scrim clipping is covered by Task 6 mounting in-window dialogs inside `AppWindowFrame` and removing `Teleport`/`fixed inset-0`.
- Compact modal refactor is covered by Task 6 shared `ModalShell` and route token update.
- Core-owned first-run detection is covered by Tasks 2 and 3.
- Missing/incomplete config repair is covered by Task 2 raw JSON completeness tests.
- Quick-start event and renderer handling are covered by Tasks 1, 3, 4, and 8.
- Skip behavior is covered by Task 8 `skip()` closing without config/service calls.
- Quick/custom quick-start modes, import, CT default, remember password, service install default, routes, DTLS, and MTU are covered by Task 8.
- Missing username/password connect prompts and remember-password sync are covered by Task 7.
- Settings select readability is covered by Task 9.

Placeholder scan:

- No placeholder markers or unspecified implementation steps are present.
- Every code-changing task includes concrete file paths, code blocks, commands, and expected outcomes.

Type consistency:

- The desktop event is named `quick-start-request` in the manifest, C++ event output, generated TypeScript event type, SSE handler, and tests.
- The config API names are `ConfigInitializationStatus`, `ConfigInitializationResult`, `ensure_initialized_config`, and `quick_start_request_data` across C++ tests and implementation.
- Renderer prompt APIs are `openQuickStart`, `closeQuickStart`, `requestCredentials`, `submitCredentialPrompt`, and `closeCredentialPrompt` across store, dialogs, and tests.
