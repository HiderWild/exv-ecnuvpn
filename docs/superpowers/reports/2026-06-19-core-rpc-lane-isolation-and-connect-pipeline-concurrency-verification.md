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
- UI mode accepted-click partial correction: calibrated Win32 `SendInput` stress artifact `build/webview-acceptance/windows/mode-toggle-sendinput-calibrated.csv` recorded 1x/2x at 900 ms and 8x/9x at 450 ms with final mode matching the expected last accepted toggle and `Responding=True`.
- UI mode idle stability correction: `build/webview-acceptance/windows/post-stress-idle-stability.csv` recorded 30 consecutive 100 ms samples at `378x148`, `Responding=True`, with zero size changes after the stress pass.
- Rapid-toggle envelope partial correction: `build/webview-acceptance/windows/mode-toggle-rapid-envelope-summary.csv`, `mode-toggle-rapid-envelope-clicks.csv`, and `mode-toggle-rapid-envelope-idle.csv` recorded packaged Win32 `SendInput` bursts at 450/150/120/80/40 ms. All bursts recorded `Responding=True` and zero idle size changes after settling.
- Connect failure visibility partial correction: `pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/ui-mode-and-connect-failure.test.ts` passed 5/5.
- Connect quick-validation correction: `./build-windows/cpp/exv.exe desktop-rpc vpn.connect "{}"` returned `ok=false`, `code=auth_failed`, and `error="VPN password is not configured."`; the runtime log recorded `connect-timing` `finish.error stage=quick_validation`.
- Lane isolation refresh: `cmake --build --preset windows-release --target core_process_lifecycle_test` passed, and `ctest --test-dir build-windows/cpp -R "core_process_lifecycle_test" --output-on-failure` passed after extending E2.3 to cover `logs.list`, `status.get`, and `config.getSettings` while `vpn.connect` remains blocked.
- Connect workflow contract refresh: `cmake --build --preset windows-release --target connect_pipeline_test connect_intent_test vpn_actions_test app_api_status_contract_test` passed, and `ctest --test-dir build-windows/cpp -R "connect_pipeline_test|connect_intent_test|vpn_actions_test|app_api_status_contract_test" --output-on-failure` passed.

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
- Post-review: `ui-mode-and-connect-failure.test.ts` is now part of `pnpm --dir webui test:host`, covering frontend-owned mode/prompt storage, stale mode-write suppression, connection failure presentation contracts, and in-progress cancellation wiring through AST/structural checks rather than exact source-fragment matching.
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
- UIA automation is not accepted as evidence for compact-mode toggle parity because the WebView accessibility tree can expose only pane nodes and traversal can block. Calibrated Win32 mouse input is the current accepted evidence source for packaged mode toggling.
- Calibrated Win32 mouse input proved final mode equals the last accepted toggle for 1x/2x 900 ms and 8x/9x 450 ms sequences. Faster 220 ms, 120 ms, and 60 ms physical bursts cannot be evaluated by raw click-count parity because the 340 ms resize shield intentionally captures input while the window is settling; full interactive rapid-toggle parity remains unchecked.
- The packaged rapid-toggle envelope run showed 450 ms baselines with 8 and 9 observed transitions ending in the expected stable mode. Shielded bursts at 150/120/80/40 ms produced fewer observed transitions than physical clicks, as expected when the resize shield captures input, and then remained stable with no idle size changes. This supports the no-bounce behavior but still leaves product-level acceptance semantics for very fast physical bursts to interactive verification.
- A post-stress idle probe showed no delayed bounce: every 100 ms sample for 2.9 seconds remained `378x148` and the shell process remained `Responding=True`.
- Frontend failure presentation is partially covered by `ui-mode-and-connect-failure.test.ts`: backend connection failures remain visible and `user_cancelled` is guarded from `setError`. A concrete desktop quick-validation failure produces the canonical `auth_failed` envelope and timing log, but this does not prove behavior for a real gateway-authentication failure.
- Read-only lane isolation is covered by `core_process_lifecycle_test` E2.3: with a deterministic hook blocking the background `vpn.connect` job, `logs.list`, `status.get`, and `config.getSettings` each respond within 500 ms before the hook is released.
- First-failure behavior is covered by `connect_pipeline_test`: the first failing branch returns before slow branches release, losing branches receive cancellation, late non-cancel failures are logged once with the first-failure reason, and successful late branch results are discarded silently.
- User cancellation and latest-intent coalescing are covered by `connect_intent_test` and `vpn_actions_test`: user-cancelled disconnects do not set `last_error`, repeated connect/cancel clicks maintain one active workflow, a newer connect starts once after cleanup, and failed jobs do not retry without a later user epoch.
- Connect timing branch visibility is covered by `app_api_status_contract_test`: source guards require `desktop.connect.backend_helper_ready`, `desktop.connect.platform_ready`, `desktop.connect.protocol_handshake`, `first_failure`, and `serial_tail` timing markers.

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

Support tool:

- `scripts/manual-phase7-vpn-verification.ps1` captures the remaining interactive Phase 7 evidence without changing VPN configuration or collecting credentials. It can launch the packaged Windows UI, or run with `-NoLaunch` while an existing UI is already open, sample `exv-ui`/`exv`/`exv-helper` process responsiveness without process command lines, save a redacted connect-stage log summary by default, and create `manual-observation.md` under `build/manual-verification/`. Raw current-session log delta capture is opt-in via `-IncludeRawLogDelta` and is redacted before writing. Use this when reproducing the reported stuck/non-responsive state so the process samples and log summary are attached to the observation.

Partially run:

- Packaged Windows WebView shell launch.
- Packaged Windows WebView shell launch responsiveness, with `exv-ui` staying `Responding=True` for 8 seconds after the UI-thread event-pump correction.
- Rapid packaged UI mode toggling automation after the correction showed no late mode bounce and `Responding=True`. The earlier coordinate-based pass was discarded because it missed the advanced WebView toggle; the later calibrated Win32 `SendInput` pass is retained as evidence for last accepted toggle parity at 450 ms and slower.
- Core/user trace search for `window.setMode`, with zero matches in current app log, core IPC registry, and acceptance artifact folder.

The following still require interactive local VPN/UI operation with a real in-progress connection and were not executed by this agent:

- Real connect against the VPN gateway.
- Open logs while a real VPN connect is in progress.
- Confirm logs/status/config do not wait for VPN connect completion during real interactive use. Automated core isolation for all three request families is verified.
- Confirm first-failure branch errors reach the visible UI before slower branch cleanup finishes during real interactive use. Automated pipeline first-failure behavior is verified.
- Confirm user cancellation during a real connect does not show an error modal or failure log. Automated cancellation contract is verified.
- Confirm rapid connect/cancel clicking coalesces to the latest user intent in the packaged UI. Automated intent coalescing is verified.
- Confirm rapid 8+ minimal/advanced clicks interactively, especially whether physical clicks during the 340 ms resize shield should be ignored or queued for a later accepted state change.
- Confirm real gateway-authentication failure is visible to the user. The automated frontend-visible failure contract and missing-password quick-validation envelope are verified, but a real VPN gateway failure was not exercised.
- Confirm real connect timing output in application logs. Automated source guards for branch timing markers are verified.
