# G3 macOS Platform Validation

> Date: 2026-05-21
> Branch: integration/platform-convergence-next
> Owner: team-lead

> Supersession note (2026-05-22): this document remains valid as historical macOS automated validation evidence, but its final recommendation is superseded by `docs/superpowers/plans/2026-05-22-develop-merge-and-release-readiness.md`. The current gate requires Windows validation and macOS manual helper-installed plus helper-missing functional evidence before merging to `develop`.

## Automated Validation Results

### Native Build
- `cmake --build --preset macos-release`: **PASS**
- All C++ targets compiled without errors or warnings

### Test Suite
- `ctest --test-dir build/macos/cpp --output-on-failure`: **5/5 PASS**
  - platform_status_models_test: PASS
  - vpn_runtime_test: PASS
  - tunnel_script_contract_test: PASS
  - app_api_runtime_policy_test: PASS
  - crypto_roundtrip_test: PASS

### Webui Build
- `npm run build`: **PASS** (201ms)

### Electron Build
- `npm run build:electron`: **PASS**

## Manual Validation (Requires User)

The following manual tests require actual VPN credentials and network access:

1. **macOS helper-installed connect**: Launch app, click connect, verify VPN tunnel establishes
2. **macOS helper-installed disconnect**: Click disconnect, verify tunnel tears down cleanly
3. **macOS helper-missing one-time elevated connect**: Uninstall helper, click connect, verify elevated prompt appears and connection works
4. **macOS helper-missing one-time disconnect**: While connected via elevated path, click disconnect, verify clean teardown
5. **Service page status**: Verify service page shows correct installed/running/available status
6. **Legacy launchd migration**: If `/Library/LaunchDaemons/com.ecnu.exv.helper.plist` still points to `/usr/local/bin/exv __helper-daemon`, the service page must warn that this is a legacy daemon and require reinstall/migration to `/usr/local/bin/exv-helper --service`

## Windows Validation (Requires Windows Machine)

Windows validation (G3.1/G3.2) cannot be performed from macOS. Requires:
1. Windows build: `cmake --build --preset windows-release`
2. Windows tests: `ctest --preset windows-release`
3. Windows helper-installed connect/disconnect
4. Windows service page status

## Summary

| Validation | Result |
|-----------|--------|
| macOS native build | PASS |
| macOS test suite (5 tests) | PASS |
| Webui build | PASS |
| Electron build | PASS |
| macOS functional VPN test | PENDING (requires user) |
| Windows build + tests | PENDING (requires Windows) |

**Historical recommendation, superseded on 2026-05-22**: this earlier recommendation to proceed with G4 based on automated macOS validation alone is no longer sufficient. Follow `docs/superpowers/plans/2026-05-22-develop-merge-and-release-readiness.md` for the active merge gate.
