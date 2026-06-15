# Config Platform Return and C++20 Module Pilot Plan

Date: 2026-06-15

## Summary

This plan applies the same corrected boundary used by the helper platform
return slice to core config only. Config remains a core domain for loading,
saving, validating, and exposing user configuration. Platform-specific config
defaults and system path details belong under `src/platform`, not under
`src/core/config/platform`.

This slice does not change config RPC actions, manifest content, persisted JSON
shape, credential storage behavior, route configuration semantics, helper
behavior, or VPN runtime behavior.

## Implementation Tasks

1. Move config platform defaults back to platform:
   - `ConfigDefaults` and `config_defaults()` declaration live in
     `src/platform/common/config_defaults.hpp`.
   - OS implementations live in
     `src/platform/{win32,darwin,linux}/config_defaults.cpp`.
   - Config code and tests include `platform/common/config_defaults.hpp`.

2. Enforce the boundary with tests:
   - `src/core/config/platform` must not exist.
   - `CMakeLists.txt` must not compile `src/core/config/platform` sources.
   - Source and test files must not include `core/config/platform/...`.

3. Add the config C++20 named module pilot:
   - Add `exv.config.contract` as a narrow module facade.
   - Export only read-only config contract helpers for canonical actions and
     legacy aliases.
   - Keep JSON serialization, persistence, runtime policy, and RPC dispatch in
     existing headers and sources.
   - Add `config_module_smoke_test` to import the module and compare the
     exported action/alias helpers with the generated contract header.

4. Preserve compatibility:
   - Keep existing namespaces for this slice.
   - Do not add production mocks or stubs.
   - Do not change config action names, alias mappings, payloads, or errors.

## Verification

Focused verification:

```powershell
cmake --build build --target contract_manifest_test config_module_smoke_test config_actions_test runtime_status_native_test native_engine_contract_test native_event_sink_test tunnel_script_contract_test app_api_runtime_policy_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(contract_manifest_test|config_module_smoke_test|config_actions_test|runtime_status_native_test|native_engine_contract_test|native_event_sink_test|tunnel_script_contract_test|app_api_runtime_policy_test)$"
```

Static checks:

```powershell
if (Test-Path src\core\config\platform) { exit 1 }
rg -n '#include "core/config/platform/' src tests
rg -n 'src/core/config/platform' CMakeLists.txt
```

Final verification:

```powershell
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
git diff --check
```

## Notes For Future Agents

The current ownership rule is: config owns configuration domain behavior;
platform owns OS-specific defaults and system integration details. Do not
recreate `src/core/config/platform` when adding new platform-specific config
behavior.
