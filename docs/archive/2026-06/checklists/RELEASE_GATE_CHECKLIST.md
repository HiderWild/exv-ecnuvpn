# Release Gate Checklist

## Build
- [ ] Windows: `cmake --preset windows-release && cmake --build --preset windows-release`
- [ ] Linux: `cmake --preset linux-release && cmake --build --preset linux-release`
- [ ] macOS: `cmake --preset macos-release && cmake --build --preset macos-release`

## Tests
- [ ] `ctest --preset windows-release --output-on-failure`
- [ ] `ctest --preset linux-release --output-on-failure`
- [ ] `ctest --preset macos-release --output-on-failure`
- [ ] `ctest -L architecture` passes
- [ ] `ctest -L security` passes

## Architecture Guardrails
- [ ] `scripts/architecture-guardrails.sh` passes (Linux/macOS)
- [ ] `scripts/architecture-guardrails.ps1` passes (Windows)

## Security
- [ ] `no_secret_in_argv_test` passes
- [ ] `no_secret_in_logs_test` passes
- [ ] `credential_store` tests pass
- [ ] No passwords/cookies/tokens in helper code
- [ ] No platform `#ifdef` in core layer

## Integration
- [ ] `native_core_connect_flow_test` passes
- [ ] `helper_timeout_cleanup_test` passes
- [ ] `auth_failure_test` passes
- [ ] `helper_lost_test` passes

## UI
- [ ] `cd webui && npm run typecheck`
- [ ] `cd webui && npm run build`

## Manual Verification
- [ ] Can connect to VPN (requires real server)
- [ ] Can disconnect from VPN
- [ ] Auto-reconnect works
- [ ] Helper timeout cleanup works
- [ ] Credential store saves/loads secrets

## Notes
- Architecture guardrails check that helper code does not contain credentials,
  core code does not have platform-specific ifdefs, and protocol boundaries
  are respected.
- Integration tests use FakeHelper and FakePlatformNetworkOps and do not
  require a real VPN server.
