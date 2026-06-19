# Connect Pipeline Post-Audit Corrections

Date: 2026-06-19

Source: post-implementation `gpt-5.3-codex-spark` sub-agent review, then main-agent review.

## Correction 1: Discarded Prepared Handshake Cleanup

Finding:

- A prepared native handshake could be produced by the protocol branch and then discarded when another connect-pipeline branch failed first.
- The losing path relied on object destruction rather than explicit `ProtocolSession::disconnect()` / `ProtocolTransport::disconnect()`.
- `NativeVpnEngineSession::stop()` also did not explicitly disconnect an adopted handshake if packet attach had not started.

Resolution:

- Added explicit disconnect behavior in `NativeVpnEngineSession::stop()` for adopted handshakes that have not been moved into the packet loop.
- Added RAII cleanup to the desktop prepared-handshake holder so a successful handshake discarded before controller handoff is explicitly disconnected.
- Added a native engine contract test proving adopted handshakes are disconnected on `stop()` before packet attach.
- Fixed a flaky late-failure wait in `connect_pipeline_test` that could block the late logger while sleeping.

Verification:

- `cmake --build --preset windows-release --target connect_pipeline_test native_engine_contract_test exv app_api_status_contract_test`
- `ctest --test-dir build-windows/cpp -R "app_api_status_contract_test|connect_pipeline_test|native_engine_contract_test|native_handshake_job_test|core_session_runner_test|tunnel_controller_integration_test" --output-on-failure`

Commit:

- `9801321 core: close discarded prepared handshakes`

## Correction 2: Verification Report Manual Items

Finding:

- The checklist correctly left nine manual UI/VPN reproduction items unchecked.
- The verification report initially listed only a subset of those manual items, weakening traceability.

Resolution:

- Expanded the verification report's manual-not-run section so every unchecked Phase 7 manual item is represented.

Verification:

- Manual comparison between the Phase 7 checklist and the report's manual-not-run section.

Commit:

- `c5fbbec docs: list all pending manual verification items`

## Correction 3: Missing Plan-Referenced Test Entrypoints

Finding:

- The plan and checklist referenced `ui_shell_async_host_bridge_test`, but the repository only had async-host coverage embedded inside `ui_shell_runtime_test`; the named CMake/CTest target did not exist.
- The plan referenced `webui/host/__tests__/ui-mode-and-connect-failure.test.ts`, but no such host test existed and `pnpm --dir webui test:host` could not exercise that named acceptance gate.
- The plan's focused Phase 13 command referenced `connection_attempt_test`, but `tests/connection_attempt_test.cpp` was not registered as a CMake/CTest target. Once registered, it exposed stale test code and a real feedback-code regression where `connection_attempt_active` collapsed to `connection_failed`.

Resolution:

- Added standalone `tests/ui_shell_async_host_bridge_test.cpp`, registered it in CMake, and included it in release-blocking tests.
- Added `webui/host/__tests__/ui-mode-and-connect-failure.test.ts` and included it in `pnpm --dir webui test:host`.
- Reworked that host test from brittle exact source-fragment matching to TypeScript AST/structural checks while keeping it as a lightweight host-contract gate.
- Registered `connection_attempt_test` as a CMake/CTest target, fixed its stale helper-pid and source-path assumptions, and updated its static guard for the current accepted-job connect flow.
- Added `connection_attempt_active` as a canonical feedback code and synchronized the frontend VPN error type/presentation map.

Verification:

- `cmake --build --preset windows-release --target core_rpc_lane_scheduler_test connect_intent_test connect_pipeline_test core_process_lifecycle_test ui_shell_core_rpc_client_test ui_shell_runtime_test ui_shell_async_host_bridge_test app_api_rpc_dispatcher_test core_architecture_contract_test vpn_actions_test connection_attempt_test win32_driver_status_test feedback_test`
- `ctest --test-dir build-windows/cpp -R "core_rpc_lane_scheduler_test|connect_intent_test|connect_pipeline_test|core_process_lifecycle_test|ui_shell_core_rpc_client_test|ui_shell_runtime_test|ui_shell_async_host_bridge_test|app_api_rpc_dispatcher_test|core_architecture_contract_test|vpn_actions_test|connection_attempt_test|win32_driver_status_test|feedback_test" --output-on-failure`
- `pnpm --dir webui test:host`

Commit:

- Pending until commit is created.
