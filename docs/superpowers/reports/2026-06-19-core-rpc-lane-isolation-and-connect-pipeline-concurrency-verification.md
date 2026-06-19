# Core RPC Lane Isolation And Connect Pipeline Concurrency Verification

Date: 2026-06-19 04:51 +08:00

## Automated Verification

Passed:

- `ctest --test-dir build-windows/cpp -R "app_api_status_contract_test|core_process_lifecycle_test|connect_intent_test|connect_pipeline_test|vpn_actions_test|core_session_runner_test|native_engine_contract_test|native_handshake_job_test|win32_driver_status_test|app_api_runtime_policy_test" --output-on-failure`
- `pnpm --dir webui test:host`
- `pnpm --dir webui exec vue-tsc -b`
- `pnpm --dir webui run build`
- `./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking`
- Post-review resource cleanup fix: `cmake --build --preset windows-release --target connect_pipeline_test native_engine_contract_test exv app_api_status_contract_test`
- Post-review resource cleanup fix: `ctest --test-dir build-windows/cpp -R "app_api_status_contract_test|connect_pipeline_test|native_engine_contract_test|native_handshake_job_test|core_session_runner_test|tunnel_controller_integration_test" --output-on-failure`
- Post-review test-entry correction: `cmake --build --preset windows-release --target core_rpc_lane_scheduler_test connect_intent_test connect_pipeline_test core_process_lifecycle_test ui_shell_core_rpc_client_test ui_shell_runtime_test ui_shell_async_host_bridge_test app_api_rpc_dispatcher_test core_architecture_contract_test vpn_actions_test connection_attempt_test win32_driver_status_test feedback_test`
- Post-review test-entry correction: `ctest --test-dir build-windows/cpp -R "core_rpc_lane_scheduler_test|connect_intent_test|connect_pipeline_test|core_process_lifecycle_test|ui_shell_core_rpc_client_test|ui_shell_runtime_test|ui_shell_async_host_bridge_test|app_api_rpc_dispatcher_test|core_architecture_contract_test|vpn_actions_test|connection_attempt_test|win32_driver_status_test|feedback_test" --output-on-failure`
- Post-review frontend contract correction: `pnpm --dir webui test:host`
- Current HEAD refresh: `cmake --build --preset windows-release --target core_rpc_lane_scheduler_test connect_intent_test connect_pipeline_test core_process_lifecycle_test ui_shell_core_rpc_client_test ui_shell_runtime_test ui_shell_async_host_bridge_test app_api_rpc_dispatcher_test core_architecture_contract_test vpn_actions_test connection_attempt_test win32_driver_status_test feedback_test`
- Current HEAD refresh: `ctest --test-dir build-windows/cpp -R "core_rpc_lane_scheduler_test|connect_intent_test|connect_pipeline_test|core_process_lifecycle_test|ui_shell_core_rpc_client_test|ui_shell_runtime_test|ui_shell_async_host_bridge_test|app_api_rpc_dispatcher_test|core_architecture_contract_test|vpn_actions_test|connection_attempt_test|win32_driver_status_test|feedback_test" --output-on-failure`
- Current HEAD refresh: `pnpm --dir webui test:host`
- Current HEAD refresh: `pnpm --dir webui exec vue-tsc -b`
- Current HEAD refresh: `pnpm --dir webui run build`
- Current HEAD refresh: `./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking`

Release-blocking result: 74/74 tests passed after clearing stale local `exv-ui`/`exv-helper` processes that were locking `build-windows/cpp/exv-helper.exe` during the first build attempt.

## Verified Behaviors

- Desktop `vpn.connect` returns accepted job state promptly while heavy work runs in the background.
- Core RPC lane scheduling keeps read/log/config work isolated from VPN control work.
- Desktop connect now runs backend/helper readiness, platform readiness, and native protocol handshake as pipeline branches before entering the controller serial tail.
- Route/DNS/adapter mutation remains in the controller network-config serial tail after readiness branches succeed.
- Native auth/CSTP handshake can be prepared once and adopted by `CoreSessionRunner` without re-authentication or a second CSTP connect.
- Windows driver readiness uses one cached adapter snapshot for burst status/preflight calls instead of separate Wintun and TAP adapter scans.
- `minimal_mode` and `service_install_prompt_seen` are frontend-local values backed by `localStorage`; remote settings payloads cannot overwrite them.
- Connect timing source guards cover backend/helper, platform, protocol handshake, first failure, and serial tail markers.
- Post-review: adopted/prepared native handshake resources are explicitly disconnected if stopped before packet attach or discarded before controller handoff.
- Post-review: `ui_shell_async_host_bridge_test` is now a real CMake/CTest target covering non-blocking host dispatch, local `window.setMode`, invalid host requests, and shutdown suppression of late core replies.
- Post-review: `ui-mode-and-connect-failure.test.ts` is now part of `pnpm --dir webui test:host`, covering frontend-owned mode/prompt storage, stale mode-write suppression, visible connect failures, and in-progress cancellation wiring through AST/structural checks rather than exact source-fragment matching.
- Post-review: `connection_attempt_test` is now a real CMake/CTest target, and `connection_attempt_active` is preserved as a canonical backend/frontend error code instead of collapsing to `connection_failed`.

## Implementation Commits

Base reviewed commit before latest post-audit corrections:

- `355964b docs: record post-audit connect corrections`

Latest post-audit test-entry correction commit:

- `d030038 test: restore connect pipeline acceptance gates`

Follow-up correction after `d030038`:

- Removed remaining exact source-fragment assertions from `ui-mode-and-connect-failure.test.ts`; the host test now uses AST/structural checks for the frontend-local and cancellation contracts.
- Commit: `dc18d2b test: harden frontend mode contract gate`

## Manual Verification Not Run

The following require interactive local VPN/UI operation and were not executed by this agent:

- Real connect against the VPN gateway.
- Open logs while a real VPN connect is in progress.
- Rapidly toggle minimal/advanced mode in the packaged UI and visually confirm the final mode equals the last click.
- Confirm logs/status/config do not wait for VPN connect completion during real interactive use.
- Confirm first-failure branch errors reach the visible UI before slower branch cleanup finishes.
- Confirm user cancellation during a real connect does not show an error modal or failure log.
- Confirm rapid connect/cancel clicking coalesces to the latest user intent in the packaged UI.
- Confirm connect failure is visible to the user.
- Confirm `window.setMode` does not appear in real core RPC traces.
- Confirm real connect timing output in application logs.
