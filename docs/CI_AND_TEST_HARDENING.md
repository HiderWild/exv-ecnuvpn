# CI and Test Hardening

## CI Workflow Structure

The GitHub Actions workflow (`.github/workflows/build.yml`) runs on every push
and PR to `main` and `develop`. It builds and tests on three platforms:

| Job | Runner | Preset | Build Dir |
|---|---|---|---|
| `build-windows` | `windows-latest` | `windows-release` | `build-windows/cpp` |
| `build-linux` | `ubuntu-latest` | `linux-release` | `build/linux/cpp` |
| `build-macos` | `macos-13` + `macos-latest` | `macos-release` | `build/macos/cpp` |

Each job:
1. Checks out code
2. Builds the WebUI frontend (Node 20 + Vite)
3. Runs `cmake --preset <preset>`
4. Runs `cmake --build --preset <preset>`
5. Runs `ctest --preset <preset> --output-on-failure` (failures are non-blocking via `continue-on-error`)
6. On failure, uploads `Testing/Temporary/LastTest.log` and CMake logs as artifacts

### Debugging CI Failures

Download the artifact for the failing platform. The key files are:
- `Testing/Temporary/LastTest.log` -- per-test output with pass/fail/timeout details
- `CMakeOutput.log` -- CMake configure output
- `CMakeError.log` -- CMake configure errors (missing deps, etc.)

## MinGW DLL Issue

### Problem

On Windows with the MinGW toolchain, test executables that link against
`exv-core` (which pulls in nlohmann/json headers and platform libs) may crash
with exit code `0xc0000139` (STATUS_ENTRYPOINT_NOT_FOUND). This is a DLL
loading error: the MinGW runtime DLLs (`libgcc_s_seh-1.dll`,
`libstdc++-6.dll`, etc.) are not on PATH when CTest spawns the test process.

### Affected Tests

- `helper_cleanup_registry_test`
- `helper_contract_test`
- `helper_messages_connector_test`

### Fix

The CMakeLists.txt injects the build directory of `exv-core` into the `PATH`
environment variable for each affected test via the CMake `ENVIRONMENT` test
property:

```cmake
if(WIN32 AND NOT MSVC)
    set_tests_properties(helper_cleanup_registry_test PROPERTIES
        ENVIRONMENT "PATH=$<TARGET_FILE_DIR:exv-core>;$ENV{PATH}"
    )
endif()
```

This ensures the MinGW runtime DLLs (which are placed next to the test
binaries by the build system) are found at load time.

### Skipping Env-Sensitive Tests

Tests that are known to be affected by this issue are labeled `env-sensitive`.
To skip them on a broken toolchain:

```bash
ctest --preset windows-release -LE env-sensitive
```

## Test Labels

All Phase 1+ architecture tests carry one or more labels for selective running:

| Label | Tests | Purpose |
|---|---|---|
| `architecture` | All 13 Phase 1+ tests | Run all new architecture tests |
| `core` | `reconnect_policy_test`, `core_error_mapper_test`, `tunnel_controller_state_machine_test`, `tunnel_controller_integration_test` | Core module tests |
| `helper` | `helper_lease_manager_test`, `helper_cleanup_registry_test`, `helper_contract_test`, `helper_messages_connector_test`, `helper_timeout_cleanup_test` | Helper subsystem tests |
| `platform` | `platform_network_ops_model_test` | Platform abstraction tests |
| `integration` | `native_core_connect_flow_test`, `tunnel_controller_integration_test`, `helper_timeout_cleanup_test` | Cross-module integration tests |
| `ui-contract` | `app_api_rpc_dispatcher_test` | UI/RPC contract tests |
| `security` | `no_secret_in_argv_test` | Security-focused tests |
| `env-sensitive` | `helper_cleanup_registry_test`, `helper_contract_test` | MinGW DLL-dependent; skip on broken toolchains |

### Running Labelled Tests

```bash
# All architecture tests
ctest --preset windows-release -L architecture

# Only core tests
ctest --preset linux-release -L core

# Only helper tests
ctest --preset macos-release -L helper

# Skip env-sensitive tests
ctest --preset windows-release -LE env-sensitive

# Integration tests only
ctest --preset linux-release -L integration
```

## Local Test Runners

### PowerShell (Windows)

```powershell
# Default preset
.\scripts\run-tests.ps1

# Specific preset
.\scripts\run-tests.ps1 -Preset windows-release

# Run only architecture tests
.\scripts\run-tests.ps1 -Label architecture

# Run only core tests
.\scripts\run-tests.ps1 -Label core
```

### Bash (Linux / macOS)

```bash
# Default preset (linux-release)
./scripts/run-tests.sh

# Specific preset
./scripts/run-tests.sh macos-release

# Run only helper tests
./scripts/run-tests.sh linux-release helper
```

## Adding New Tests

When adding a new test:

1. Add the `add_executable()` and `add_test()` in `CMakeLists.txt`
2. If the test links against `exv-core` on MinGW, add the `ENVIRONMENT` fix
3. Add the test to the appropriate `set_tests_properties()` label blocks
4. If the test is env-sensitive, add it to the `env-sensitive` label group
