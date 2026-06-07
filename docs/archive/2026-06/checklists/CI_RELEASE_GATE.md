# CI Release Gate

This document describes the ECNU-VPN CI test label system, how the release gate works, and how to add new tests.

## Test Label Taxonomy

Every test in CMakeLists.txt carries one or more **release labels** and zero or more **functional labels**.

### Release Labels (CI gate)

| Label | Meaning | CI Behavior |
|---|---|---|
| `release-blocking` | Core logic tests that must pass | **Blocks PR merge** |
| `env-sensitive` | Platform/DLL-dependent tests | Informational, `continue-on-error` |
| `integration` | Cross-module integration tests | Informational, `continue-on-error` |
| `elevated-manual` | Requires admin/root privileges | Not run in CI |
| `legacy` | Pre-existing tests with known issues | Not run in CI |

### Functional Labels (local dev filtering)

| Label | Scope |
|---|---|
| `architecture` | Phase 1+ architecture tests |
| `security` | Security-focused tests |
| `core` | Core module tests |
| `helper` | Helper subsystem tests |
| `platform` | Platform abstraction tests |
| `ui-contract` | UI / RPC contract tests |

Tests can carry multiple labels. For example, `no_secret_in_argv_test` is labeled `release-blocking`, `architecture`, and `security`.

## How CI Works

The GitHub Actions workflow (`.github/workflows/build.yml`) runs on every push and PR:

1. **WebUI build** -- `npm install && npm run build`
2. **CMake configure + build**
3. **Architecture guardrails** -- runs `scripts/architecture-guardrails.sh` (Linux/macOS) or `scripts/architecture-guardrails.ps1` (Windows)
4. **DLL diagnostic** (Windows only) -- runs `scripts/diagnose-mingw-dlls.ps1`
5. **Release-blocking tests** -- `ctest -L release-blocking` -- **must pass**
6. **Env-sensitive tests** -- `ctest -L env-sensitive` -- informational, `continue-on-error`
7. **Integration tests** -- `ctest -L integration` -- informational, `continue-on-error`
8. **Upload test logs** -- always (not just on failure)

If the release-blocking step fails, the CI job fails and the PR cannot be merged.

## Running Tests Locally

### PowerShell (Windows)

```powershell
# Run all tests
scripts\run-tests.ps1

# Run only release-blocking tests
scripts\run-tests.ps1 -Label release-blocking

# Run only env-sensitive tests
scripts\run-tests.ps1 -Label env-sensitive

# List all available labels
scripts\run-tests.ps1 -ListLabels

# Run DLL diagnostics before tests
scripts\run-tests.ps1 -Diagnostics
```

### Bash (Linux/macOS)

```bash
# Run all tests
scripts/run-tests.sh linux-release

# Run only release-blocking tests
scripts/run-tests.sh linux-release --label release-blocking

# List all available labels
scripts/run-tests.sh linux-release --list-labels

# Run platform diagnostics
scripts/run-tests.sh linux-release --diagnostics
```

### Direct CMake/CTest

```bash
# Configure
cmake --preset windows-release

# Build
cmake --build --preset windows-release

# Run release-blocking only
ctest --preset windows-release -L release-blocking --output-on-failure

# Run everything except env-sensitive
ctest --preset windows-release -LE env-sensitive --output-on-failure

# Show all test labels
ctest --preset windows-release --print-labels
```

## How to Add a New Test with Proper Labels

1. Add the test target in `CMakeLists.txt`:

```cmake
add_executable(my_new_test tests/my_new_test.cpp src/my_module.cpp)
target_include_directories(my_new_test PRIVATE
    ${CMAKE_SOURCE_DIR}/include
    ${CMAKE_SOURCE_DIR}/src
)
add_test(NAME my_new_test COMMAND my_new_test)
```

2. Add the release label:

```cmake
# For core logic tests (most common):
set_property(TEST my_new_test APPEND PROPERTY LABELS "release-blocking")

# For platform-specific tests that need DLLs:
set_property(TEST my_new_test APPEND PROPERTY LABELS "env-sensitive")

# For integration tests:
set_property(TEST my_new_test APPEND PROPERTY LABELS "integration")
```

3. Add functional labels (optional, for local dev filtering):

```cmake
set_property(TEST my_new_test APPEND PROPERTY LABELS "architecture")
set_property(TEST my_new_test APPEND PROPERTY LABELS "security")
```

4. If the test has MinGW DLL issues on Windows, add the PATH fix:

```cmake
if(WIN32 AND NOT MSVC)
    set_tests_properties(my_new_test PROPERTIES
        ENVIRONMENT "PATH=$<TARGET_FILE_DIR:exv-core>;$ENV{PATH}"
    )
endif()
```

## MinGW DLL Issue Root Cause

On Windows, the MinGW toolchain links against runtime DLLs (`libstdc++-6.dll`, `libgcc_s_seh-1.dll`, `libwinpthread-1.dll`). These DLLs must be on PATH when test executables run.

### Symptoms

- Test exits with code `0xc0000139` (DLL not found)
- Test appears as "Not Run" in CTest output
- `dumpbin /dependents test.exe` shows MinGW DLLs as dependencies

### Root Cause

The MinGW `bin` directory (e.g., `C:\mingw64\bin`) is on PATH during compilation but may not be on PATH when CTest runs tests in a separate process.

### Fixes (in order of preference)

1. **CMake ENVIRONMENT property** (best for individual tests):
   ```cmake
   if(WIN32 AND NOT MSVC)
       set_tests_properties(my_test PROPERTIES
           ENVIRONMENT "PATH=$<TARGET_FILE_DIR:exv-core>;$ENV{PATH}"
       )
   endif()
   ```

2. **Global PATH fix** (for CI):
   ```yaml
   - name: Add MinGW to PATH
     run: echo "C:\mingw64\bin" >> $GITHUB_PATH
   ```

3. **Local dev fix**:
   ```powershell
   $env:PATH = "C:\mingw64\bin;$env:PATH"
   ctest --preset windows-release
   ```

### Diagnostic Tool

Run the DLL diagnostic to identify which tests have missing dependencies:

```powershell
scripts\diagnose-mingw-dlls.ps1 -BuildDir build-windows/cpp
```

## Recovery Plan for Env-Sensitive Test Skips

When an env-sensitive test is consistently failing in CI:

1. **Diagnose**: Run `scripts/diagnose-mingw-dlls.ps1` to identify the missing DLL
2. **Fix**: Apply the CMake ENVIRONMENT property fix (see above)
3. **Promote**: Move the test from `env-sensitive` to `release-blocking` once it passes
4. **Document**: Update this file with the fix details

If a test cannot be fixed (e.g., requires hardware or elevated privileges), move it to `elevated-manual` and document why.
