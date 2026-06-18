# WebView Shell Remediation Acceptance - 2026-06-18

## Branches

- Target branch: `codex/ui-framework-webview-shell`
- Merged source: `codex/cli-core-ui-contract-refactor`
- Included source lineage: `codex/native-only-cutover`

## Scope Closed

- Integrated the native-only remediation into the CLI contract refactor branch, then merged that branch into the WebView shell branch.
- Preserved the WebView shell changes while resolving native protocol, CLI contract, generated contract, and UI error-state conflicts.
- Kept the native engine as the only VPN engine path for shell/runtime policy, with explicit native DTLS unavailable reporting until production DTLS is implemented.
- Restored the AnyConnect v2 aggregate-auth parser/session flow and fake-server fixtures required by native production transport tests.
- Kept UI contract generation, host package policy, and WebView runtime checks aligned with the generated system contract.
- Added guardrails for service maintenance RPCs so legacy `service.uninstall` does not fall through to privileged platform maintenance when no service backend is available.
- Strengthened redaction/error mapping so release-blocking security and tunnel-controller tests do not leak seeded secret details or misclassify packet-open failures.

## Verification

- `python scripts\generate_contracts.py --check`
- `cmake --build build-windows\cpp --target contract_manifest_test app_api_rpc_dispatcher_test app_api_status_contract_test ui_shell_contract_test win32_webview2_runtime_test native_aggregate_auth_test native_production_transport_test native_session_state_test native_engine_config_mapper_test native_dtls_transport_test native_engine_contract_test native_fake_anyconnect_server_test core_error_mapper_test core_session_runner_test`
- `ctest --test-dir build-windows\cpp -R "contract_manifest_test|app_api_rpc_dispatcher_test|app_api_status_contract_test|ui_shell_contract_test|win32_webview2_runtime_test|native_aggregate_auth_test|native_production_transport_test|native_session_state_test|native_engine_config_mapper_test|native_dtls_transport_test|native_engine_contract_test|native_fake_anyconnect_server_test|core_error_mapper_test|core_session_runner_test" --output-on-failure`
- `pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/desktop-contract-generated.test.ts host/__tests__/webview-package-policy.test.ts`
- `pnpm --dir webui build`
- `ctest --test-dir build-windows\cpp -R "tunnel_module_smoke_test|tunnel_controller_integration_test|service_actions_test|no_secret_in_logs_test" --output-on-failure`
- `powershell -ExecutionPolicy Bypass -File scripts\run-tests.ps1 -Preset windows-release -Label release-blocking`

All listed verification passed on the target branch after conflict resolution.

## Notes

- The final release-blocking preset passed 71/71 tests.
- `git diff --check` reported no whitespace errors; only line-ending normalization warnings were printed for touched Windows files.
