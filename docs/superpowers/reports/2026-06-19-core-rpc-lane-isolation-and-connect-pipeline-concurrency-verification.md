# Core RPC Lane Isolation And Connect Pipeline Concurrency Verification

Date: 2026-06-19 10:28 +08:00

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
- Windows IPC probe correction: `cmake --build --preset windows-release --target pipe_ipc_test core_process_lifecycle_test core_resolver_test ui_shell_core_rpc_client_test exv-cli`
- Windows IPC probe correction: `ctest --test-dir build-windows/cpp -R "pipe_ipc_test|core_process_lifecycle_test|core_resolver_test|ui_shell_core_rpc_client_test" --output-on-failure`
- Windows IPC probe correction: `./build-windows/cpp/exv-cli.exe status` returned exit code 0 with no `core_comm_broken` output.
- Windows IPC probe correction: `./build-windows/cpp/exv-cli.exe service status` returned exit code 0 with no `core_comm_broken` output.
- Windows IPC probe correction: `./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking`
- Packaged Windows WebView smoke: `./scripts/windows-packaging-smoke.ps1` passed with 13 PASS, 0 FAIL, and 4 SKIP for optional/not-running service-helper checks.
- UI responsiveness correction: `pnpm --dir webui test:host`
- UI responsiveness correction: `pnpm --dir webui exec vue-tsc -b`
- UI responsiveness correction: `ctest --test-dir build-windows/cpp -R "win32_webview2_runtime_test|ui_shell_async_host_bridge_test|ui_shell_core_rpc_client_test|core_process_lifecycle_test" --output-on-failure`
- UI responsiveness correction: `powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop`
- UI responsiveness correction: packaged `exv-ui.exe` launch stayed `Responding=True` for an 8-second Windows process probe.
- UI responsiveness correction: `powershell -ExecutionPolicy Bypass -File scripts\windows-packaging-smoke.ps1` passed with 13 PASS, 0 FAIL, and 4 SKIP.

Release-blocking result: 75/75 tests passed after adding `pipe_ipc_test`. An earlier 74/74 refresh also passed after clearing stale local `exv-ui`/`exv-helper` processes that were locking `build-windows/cpp/exv-helper.exe` during the first build attempt.

Verification note: two intermediate full release-blocking reruns failed before the final pass: one `core_process_lifecycle_test` run reported a transient `powershell.exe` lookup failure inside Windows platform checks, and one `tunnel_controller_integration_test` run exited with a Windows heap-corruption code. Both failed tests passed when rerun individually, and a subsequent full `./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking` passed 75/75.

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
- Post-review: Windows core resolver IPC probes now use strict `WaitNamedPipeA()` readiness instead of opening an empty pipe client connection, so `exv-cli status` and UI core resolution do not poison the next real named-pipe request.
- Post-review: resolver-launched daemon cores no longer inherit caller pipe handles, avoiding redirected CLI/stdout waiters being held open by the background core.
- Packaged Windows WebView shell launched from `build/windows/webview/package/ECNU VPN/exv-ui.exe` and created an `ECNU VPN` main window.
- Earlier four-click packaged UI mode switching was manually exercised successfully, but later 8+ click bursts reproduced delayed mode bounce, so the four-click result is no longer treated as sufficient final-click parity evidence.
- Rapid packaged UI stress before the correction reproduced delayed bounce at 8x@150 ms and 12x@120 ms bursts from advanced mode.
- The packaged shell also entered a Windows non-responsive state during investigation. A/B testing isolated the cause to `CoreRpcClient::pump_events()` running from the Win32 UI loop; when that pump was moved off the UI thread, the same packaged launch probe stayed `Responding=True`.
- Core event pumping now runs on a background thread and emits renderer events through the existing UI-thread `emit_event()` path. The Win32 UI loop no longer performs core event reads directly.
- `window.setMode` now carries a renderer request sequence into native hosts; stale native writes are ignored, Win32 posts the renderer response before queuing resize, and the frontend displays a 340 ms resize shield with the EXV mark while mode changes settle.
- `rg -n "window\\.setMode" $env:APPDATA\\ecnuvpn\\ecnuvpn.log $env:APPDATA\\ecnuvpn\\exv-core-ipc-v1.registry.json build\\webview-acceptance -S` returned no matches after the packaged UI mode switching pass.

## Implementation Commits

Base reviewed commit before latest post-audit corrections:

- `355964b docs: record post-audit connect corrections`

Latest post-audit test-entry correction commit:

- `d030038 test: restore connect pipeline acceptance gates`

Follow-up correction after `d030038`:

- Removed remaining exact source-fragment assertions from `ui-mode-and-connect-failure.test.ts`; the host test now uses AST/structural checks for the frontend-local and cancellation contracts.
- Commit: `dc18d2b test: harden frontend mode contract gate`

Follow-up correction after `ebfd09d`:

- Added `pipe_ipc_test`, changed the Windows resolver probe to avoid consuming a named-pipe connection before the real request, and prevented resolver-launched daemon cores from inheriting caller handles.
- Commit: recorded by the commit containing this verification update.

## Manual Verification

Partially run:

- Packaged Windows WebView shell launch.
- Packaged Windows WebView shell launch responsiveness, with `exv-ui` staying `Responding=True` for 8 seconds after the UI-thread event-pump correction.
- Rapid packaged UI mode toggling automation after the correction showed no late mode bounce and `Responding=True`, but the coordinate-based automation did not reliably hit the WebView toggle, so it does not prove final-click parity.
- Core/user trace search for `window.setMode`, with zero matches in current app log, core IPC registry, and acceptance artifact folder.

The following still require interactive local VPN/UI operation with a real in-progress connection and were not executed by this agent:

- Real connect against the VPN gateway.
- Open logs while a real VPN connect is in progress.
- Confirm logs/status/config do not wait for VPN connect completion during real interactive use.
- Confirm first-failure branch errors reach the visible UI before slower branch cleanup finishes.
- Confirm user cancellation during a real connect does not show an error modal or failure log.
- Confirm rapid connect/cancel clicking coalesces to the latest user intent in the packaged UI.
- Confirm rapid 8+ minimal/advanced clicks interactively, including final mode matching the last accepted click.
- Confirm connect failure is visible to the user.
- Confirm real connect timing output in application logs.
