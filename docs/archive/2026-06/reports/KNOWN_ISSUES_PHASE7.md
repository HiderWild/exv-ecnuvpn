# Known Issues — Phase 7 Beta Readiness

> **Last updated:** 2026-06-03
> **Purpose:** Track known issues that affect Beta release readiness.
> **Severity levels:** P0 (blocks Beta), P1 (needs workaround), P2 (known, acceptable)

---

## Severity Classification

| Severity | Definition | Beta Action |
|----------|-----------|-------------|
| **P0** | Blocks Beta release. No workaround exists. | Must be fixed before Beta ship. |
| **P1** | Degraded experience. Workaround exists. | Document workaround; fix planned for post-Beta. |
| **P2** | Known limitation. Acceptable for Beta. | Document only; fix in future phase. |

---

## Issue Registry

### ISSUE-001: MinGW DLL Path / Env-Sensitive Test Failures

- **Severity:** P1
- **Owner:** CI / Agent 1
- **Description:** 13 CTest targets fail on Windows MinGW due to missing runtime DLLs (e.g., `libstdc++-6.dll`, `libgcc_s_seh-1.dll`, `libwinpthread-1.dll`). Tests that link against `exv-core` static library pull in nlohmann/json and other headers that require the MinGW runtime DLLs at load time. On CI runners and local machines where the MinGW toolchain bin directory is not on PATH, the test executables crash with `0xc0000139` (STATUS_ENTRYPOINT_NOT_FOUND).
- **Affected tests:** `helper_cleanup_registry_test`, `helper_v2_contract_test`, `vpn_actions_test`, `helper_messages_connector_test`, and others labeled `env-sensitive`.
- **Workaround:** Add MinGW toolchain bin directory to PATH before running tests, or use `ctest -LE env-sensitive` to skip affected tests. CMakeLists.txt already sets PATH via `ENVIRONMENT` test property for some tests.
- **Planned fix:** Phase 7 — Agent 1 is diagnosing exact DLL dependencies per test and injecting PATH or copying DLLs to test binary directories.
- **Status:** In progress. See `docs/CI_RELEASE_GATE.md` for updated test labels.

### ISSUE-002: Linux Credential Store Unsupported

- **Severity:** P2
- **Owner:** Platform / Agent 3
- **Description:** The credential store backend (`src/platform/common/credential_store.cpp`) has implementations for Windows (Win Credential Store via `win_credential_store.cpp`) and macOS (Keychain via `darwin_keychain_store.cpp`), but Linux has no secure credential store backend. On Linux, credentials cannot be persisted securely.
- **Workaround:** Users must re-enter credentials each session, or use environment variables (not recommended for production).
- **Planned fix:** Post-Beta. Linux credential store could use libsecret (GNOME Keyring) or kwallet (KDE). The interface is already abstracted in `credential_store.cpp`.
- **Status:** Known gap. Documented for Beta.

### ISSUE-003: Platform Code Includes Protocol Headers (Guardrail Violation)

- **Severity:** P2
- **Owner:** Architecture / Agent 1
- **Description:** Three files in `src/platform/` include headers from `vpn_engine/protocol/`, violating the architecture boundary where platform code should not depend on protocol-layer headers:
  - `src/platform/common/vpn_supervisor_process.cpp` includes `vpn_engine/protocol/native_auth_session_json.hpp`
  - `src/platform/darwin/native_tls_stream.hpp` includes `vpn_engine/protocol/tls_stream.hpp`
  - `src/platform/win32/native_tls_stream.hpp` includes `vpn_engine/protocol/tls_stream.hpp`
- **Workaround:** None needed for Beta — these are compile-time dependencies, not runtime credential leaks. The protocol headers in question contain TLS and auth session types that are shared between layers.
- **Planned fix:** Post-Beta. Move shared types to a common layer, or use dependency injection to break the coupling.
- **Status:** Pre-existing violation. Guardrail scripts flag this as FAIL but it does not block functionality.

### ISSUE-004: Test Fixtures Contain Hardcoded Secret Strings

- **Severity:** P2
- **Owner:** Security / Testing
- **Description:** `tests/app_api_native_orchestration_test.cpp` contains password strings used in test assertions. These are test-only values (not real credentials) used to verify that the credential store correctly handles secrets. The `no_secret_in_argv_test` and `no_secret_in_logs_test` correctly identify these as test fixtures.
- **Workaround:** None needed — these are fake test values, not real campus credentials.
- **Planned fix:** Post-Beta. Consider moving test fixture secrets to a separate test-only header or using randomized test values.
- **Status:** Known warning. Does not affect runtime security.

### ISSUE-005: openconnect Binary Not Bundled in Production Packaging

- **Severity:** P1
- **Owner:** Packaging / Agent 2
- **Description:** The Electron builder config (`webui/electron-builder.config.cjs`) explicitly filters out `openconnect.exe`, `libopenconnect-*.dll`, and `libopenconnect*.dylib` from `extraResources/bin`. The legacy openconnect runtime staging scripts (`scripts/stage-openconnect-runtime-*.sh`) are diagnostic-only and require `ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1` to run. If the native VPN engine depends on openconnect for certain protocol operations, this binary must be available at runtime.
- **Workaround:** The native engine (`exv`) uses its own implementation of the AnyConnect protocol and does not require openconnect for normal operation. The legacy openconnect runtime is only for diagnostic fallback.
- **Planned fix:** If openconnect fallback is needed for Beta, update the electron-builder filter to include it. Otherwise, confirm the native engine is self-sufficient and close this issue.
- **Status:** Needs clarification from architecture owner.

### ISSUE-006: Sign and Edit Executable Disabled on Windows

- **Severity:** P2
- **Owner:** Packaging
- **Description:** The electron-builder config sets `signAndEditExecutable: false` for Windows. This means the built executable will not be code-signed. Windows SmartScreen may warn users on first launch.
- **Workaround:** Users can click "Run anyway" on the SmartScreen prompt. For trusted Beta testers, this is acceptable.
- **Planned fix:** For GA release, obtain a code signing certificate and enable `signAndEditExecutable`. Document signing requirements in the release process.
- **Status:** Known limitation for Beta. Code signing not required for Beta.

---

## Issue Template

To add a new issue, copy and fill in this template:

```markdown
### ISSUE-NNN: <Short Title>

- **Severity:** P0 | P1 | P2
- **Owner:** <Team or individual>
- **Description:** <What is the issue, how was it discovered, what is the impact>
- **Affected components:** <Files, tests, or features affected>
- **Workaround:** <How to work around it, or "None">
- **Planned fix:** <Which phase, or "Post-Beta", with brief approach>
- **Status:** <In progress | Blocked | Known gap | Deferred>
```

---

## Summary

| ID | Severity | Status | Blocks Beta? |
|----|----------|--------|-------------|
| ISSUE-001 | P1 | In progress | No (workaround: skip env-sensitive tests) |
| ISSUE-002 | P2 | Known gap | No |
| ISSUE-003 | P2 | Pre-existing | No |
| ISSUE-004 | P2 | Known warning | No |
| ISSUE-005 | P1 | Needs clarification | Possibly (needs architecture confirmation) |
| ISSUE-006 | P2 | Known limitation | No |

**Beta gate assessment:** No P0 issues currently identified. One P1 (ISSUE-001) has a workaround. One P1 (ISSUE-005) needs architecture clarification but the native engine is believed to be self-sufficient. Beta release can proceed with these issues documented.
