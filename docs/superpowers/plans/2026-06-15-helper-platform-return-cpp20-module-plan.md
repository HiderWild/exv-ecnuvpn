# Helper Platform Return and C++20 Module Pilot Plan

Date: 2026-06-15

## Summary

This plan supersedes the 2026-06-14 helper/config package consolidation
direction that placed helper-specific platform details under
`src/helper/platform`. The corrected boundary keeps helper protocol and runtime
code under `src/helper`, while all OS-specific helper transport, daemon,
service, lifecycle, endpoint, and platform network delegation code belongs
under `src/platform`.

This slice is intentionally limited to helper. It does not change helper wire
messages, heartbeat behavior, service/oneshot semantics, cleanup policy, config
contracts, or VPN engine behavior.

## Implementation Tasks

1. Move helper platform abstractions back to platform:
   - `helper_client.hpp`, `helper_lifecycle.hpp`, `helper_platform.hpp`,
     `helper_service_manager.hpp`, and `helper_delegating_network_ops.*` live in
     `src/platform/common`.
   - OS implementations live in `src/platform/{win32,darwin,linux}`.
   - Production and test includes use `platform/common/helper_...`.

2. Enforce the boundary with tests:
   - `src/helper/platform` must not exist.
   - `CMakeLists.txt` must not compile `src/helper/platform` sources.
   - Source and test files must not include `helper/platform/...`.
   - `src/platform` must not include helper runtime or handler internals.

3. Add the helper C++20 named module pilot:
   - Raise CMake minimum to 3.28.
   - Use Ninja for Linux, macOS, and Docker native builds so CMake can scan
     named module dependencies.
   - Pin CI/Docker to module-capable compilers: GCC 14 for Linux/Docker,
     Homebrew LLVM/Clang for macOS, and MSVC for Windows.
   - Add `exv.helper.protocol` as a narrow module facade for helper protocol
     enums and read-only contract helpers.
   - Add `helper_module_smoke_test` to import the module and compare exported
     op codes/session flags against the generated contract header.

4. Preserve compatibility:
   - Keep existing namespaces unless a later namespace-cleanup plan explicitly
     changes them.
   - Keep helper protocol, runtime state machine, and platform behavior
     unchanged.
   - Do not add production mocks, stubs, legacy helper paths, or versioned
     helper protocols.

## Verification

Focused verification:

```powershell
cmake --build build --target helper_contract_test helper_module_smoke_test helper_delegating_network_ops_test helper_network_ops_adapter_test helper_messages_connector_test pipe_helper_client_test backend_resolver_test app_api_runtime_policy_test tunnel_controller_integration_test win32_helper_oneshot_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(helper_contract_test|helper_module_smoke_test|helper_delegating_network_ops_test|helper_network_ops_adapter_test|helper_messages_connector_test|pipe_helper_client_test|backend_resolver_test|app_api_runtime_policy_test|tunnel_controller_integration_test|win32_helper_oneshot_test)$"
```

Final verification:

```powershell
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
git diff --check
```

## Notes For Future Agents

The archived 2026-06-14 consolidation documents are intentionally superseded.
Do not use them as the current source of truth for helper platform ownership.
The current rule is: helper owns protocol/runtime; platform owns OS-specific
helper integration.
