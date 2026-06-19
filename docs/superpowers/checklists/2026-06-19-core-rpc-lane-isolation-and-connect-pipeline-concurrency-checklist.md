# Core RPC Lane Isolation And Connect Pipeline Concurrency Checklist

Plan: `docs/superpowers/plans/2026-06-19-core-rpc-lane-isolation-and-connect-pipeline-concurrency-plan.md`

## Phase 0: Baseline And RED Tests

- [x] Confirm worktree and branch before implementation.
- [x] Run `git status --short --branch` and preserve unrelated dirty files.
- [x] Add `core_rpc_lane_scheduler_test`.
- [x] Add core process test proving `logs.list` responds while `vpn.connect` is blocked.
- [x] Add `CoreRpcClient` out-of-order response test.
- [x] Add host bridge non-blocking callback test.
- [x] Wire new tests in `CMakeLists.txt`.
- [x] Run focused RED commands and capture expected failures.
- [x] Commit RED tests.

RED evidence: commit `4ed6f86` adds the failing regression tests. `cmake --build --preset windows-release --target core_rpc_lane_scheduler_test ui_shell_core_rpc_client_test ui_shell_runtime_test core_process_lifecycle_test` fails on missing `core/rpc/lane_scheduler.hpp`, missing `CoreRpcClient::invoke_async()`, and missing `UiWindow::post_host_response()`, which are the expected Phase 1/3 implementation gaps. The follow-up focused `ctest` also reports `core_rpc_lane_scheduler_test` not run because the executable was not produced; `core_process_lifecycle_test` hit an existing `\\.\pipe\exv-core-ipc-v1` environment conflict and must be re-run after the lane implementation with no stale core owning the pipe.

## Phase 1: Metadata And Scheduler

- [x] Add `RpcLane`, `RpcConflictClass`, and `RpcActionMetadata`.
- [x] Classify all desktop/core actions into `control`, `read_model`, `vpn_control`, `config_store`, `diagnostics`, or `platform_admin`.
- [x] Add metadata registration overload to `AppRpcDispatcher`.
- [x] Preserve existing two-argument handler registration.
- [x] Update `DesktopRpcAdapter` to carry metadata.
- [x] Implement fixed lane scheduler with one worker per lane.
- [x] Prove same-lane FIFO.
- [x] Prove different-lane concurrency.
- [x] Prove repeated `vpn.connect` on the VPN lane is serialized and later coalesced by the workflow owner.
- [x] Prove scheduler shutdown behavior.
- [x] Commit metadata and scheduler.

Verification evidence: `ctest --test-dir build-windows/cpp -R "core_rpc_lane_scheduler_test|app_api_rpc_dispatcher_test" --output-on-failure` passed after commits `5b7e19b` and `8ad1371`.

## Phase 2: Core Process Front Door

- [x] Refactor `core_process_main()` so stdin/pipe threads parse and enqueue work instead of running business handlers inline.
- [x] Route native and desktop envelopes through the same lane scheduler.
- [x] Preserve response envelope shape for desktop RPC.
- [x] Preserve response envelope shape for native core RPC.
- [x] Remove global all-action desktop dispatch mutex from `core_process.cpp`.
- [x] Keep `write_json_line()` as the only stdout writer.
- [x] Add deterministic desktop VPN connect blocking hook.
- [x] Verify `logs.list` can complete while blocked `vpn.connect` is still pending.
- [x] Commit core process lane routing.

Verification evidence: after stopping the stale local `exv.exe` that owned `\\.\pipe\exv-core-ipc-v1`, `ctest --test-dir build-windows/cpp -R "core_process_lifecycle_test|core_rpc_lane_scheduler_test|app_api_rpc_dispatcher_test" --output-on-failure` passed. Commits: `a4b4790` for the deterministic hook and `9bdc571` for scheduler routing.

## Phase 3: Host And Client Async Bridge

- [x] Add `CoreRpcClient::invoke_async()`.
- [x] Add id/request_id pending map.
- [x] Add one reader pump that demultiplexes responses and events.
- [x] Preserve blocking `invoke()` as wrapper for compatibility.
- [x] Resolve all pending requests on transport close.
- [x] Add `AsyncHostBridge`.
- [x] Keep `window.setMode` and `window.resolveClosePrompt` local to shell.
- [x] Ensure host message acceptance does not wait for core.
- [x] Wire Windows WebView2 posting callback.
- [x] Wire macOS WKWebView main-thread posting callback.
- [x] Wire Linux WebKitGTK main-thread posting callback.
- [x] Commit host/client async bridge.

Verification evidence: `ctest --test-dir build-windows/cpp -R "ui_shell_core_rpc_client_test|ui_shell_runtime_test|win32_webview2_runtime_test" --output-on-failure` passed, and `cmake --build --preset windows-release --target exv-ui` passed. macOS/Linux post hooks were source-wired in their platform hosts but not compiled on this Windows runner.

## Phase 4: Parallel VPN Connect Pipeline

- [x] Add accepted-job tests for `vpn.connect`.
- [x] Add busy-connect coalescing test with `accepted=true`, `coalesced=true`, and `active_job_id`.
- [x] Add `connect_intent_test` for latest user intent and epoch reconciliation.
- [x] Add user-cancel-during-connect test where `vpn.disconnect` returns `accepted=true`, `cancelling=true`, and `user_cancelled=true`.
- [x] Prove user cancellation does not set `last_error` and does not show an error modal.
- [x] Prove connect/cancel/connect/cancel while busy results in one active workflow and latest desired intent `Disconnect`.
- [x] Prove connect/cancel/connect while busy starts exactly one new connect after cleanup reaches idle.
- [x] Prove a non-user connection failure does not retry unless a later user click increments intent epoch.
- [x] Add disconnect-during-connect responsiveness test.
- [x] Add `connect_pipeline_test` for branch concurrency.
- [x] Prove backend/helper, platform readiness, and protocol handshake branches run concurrently.
- [x] Prove first branch failure returns to UI without waiting for slower branches.
- [x] Prove cancellation is requested for losing branches after first failure.
- [x] Prove late non-cancel branch failures are logged with job id and discarded reason.
- [x] Prove successful late branch results are discarded silently after first failure.
- [x] Prove serial network tail starts only after all three branches succeed.
- [x] Create `VpnConnectJobOwner`.
- [x] Create `connect_intent.hpp/.cpp`.
- [x] Create `ConnectPipeline`.
- [x] Create `NativeHandshakeJob`.
- [x] Split native auth/CSTP handshake from packet-device and network-config attach.
- [x] Move helper/backend readiness out of the RPC request handler.
- [x] Move platform/runtime readiness out of the RPC request handler.
- [x] Return `accepted=true`, `phase=connecting`, and `job_id` promptly.
- [x] Keep route/DNS/adapter mutation in the serial tail after all readiness branches pass.
- [x] Keep `vpn.disconnect` responsive by requesting cancellation.
- [x] Wire the yellow in-progress button to `cancelConnect()`.
- [x] Make `cancelConnect()` send `vpn.disconnect` while `connectInFlight` is true.
- [x] Make UI switch immediately to the disconnected/cancelled visual state on user cancellation.
- [x] Update frontend connect state to rely on status/events after accepted response.
- [x] Commit parallel connect pipeline conversion.

Verification evidence: `ctest --test-dir build-windows/cpp -R "connect_intent_test|connect_pipeline_test|vpn_actions_test|core_rpc_lane_scheduler_test|core_process_lifecycle_test" --output-on-failure` passed for the core accepted-job/cancel primitives. `pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts`, `pnpm --dir webui test:host`, and `pnpm --dir webui exec vue-tsc -b` passed for the frontend accepted-job/cancel UI slice. `ctest --test-dir build-windows/cpp -R "native_handshake_job_test|native_engine_contract_test|connect_pipeline_test|connect_intent_test|vpn_actions_test" --output-on-failure` and `cmake --build --preset windows-release --target native_handshake_job_test connect_pipeline_test exv` passed for the native handshake boundary slice. `ctest --test-dir build-windows/cpp -R "native_engine_contract_test|native_handshake_job_test|native_event_sink_test|connect_pipeline_test|connect_intent_test|vpn_actions_test" --output-on-failure` and `cmake --build --preset windows-release --target native_engine_contract_test native_handshake_job_test native_event_sink_test exv` passed for the native handshake/packet-attach split. `ctest --test-dir build-windows/cpp -R "app_api_status_contract_test|core_process_lifecycle_test|connect_intent_test|connect_pipeline_test|vpn_actions_test|core_session_runner_test|native_engine_contract_test|native_handshake_job_test" --output-on-failure`, `cmake --build --preset windows-release --target exv`, `pnpm --dir webui test:host`, and `pnpm --dir webui exec vue-tsc -b` passed for the desktop accepted-job request-handler split. `pnpm --dir webui test:host` and `pnpm --dir webui exec vue-tsc -b` passed after adding accepted-job status polling for frontend convergence without immediate status events. `ctest --test-dir build-windows/cpp -R "native_engine_contract_test|native_handshake_job_test|native_event_sink_test|core_session_runner_test|connect_pipeline_test" --output-on-failure` passed after moving network-config preparation out of `start_packet_loop(DeviceConfig)`. `ctest --test-dir build-windows/cpp -R "connect_pipeline_test|native_handshake_job_test|native_engine_contract_test|core_session_runner_test|tunnel_controller_integration_test|app_api_status_contract_test" --output-on-failure` and `cmake --build --preset windows-release --target exv app_api_status_contract_test` passed after routing desktop connect through `ConnectPipeline`, adopting the prepared native handshake, and keeping route/DNS/adapter mutation inside the controller network-config serial tail. Commits: `fd4e3b9`, `b1b6e93`, `ba11b23`, and `4e69e0d`.

## Phase 5: Windows Platform Readiness

- [x] Add Windows driver status tests with injected adapter enumeration.
- [x] Prove preflight does not run separate Wintun and TAP enumeration when one scan is enough.
- [x] Replace double PowerShell/CIM scans with one native or single-command adapter snapshot.
- [x] Add short-lived driver status cache for connect/status bursts.
- [x] Invalidate driver cache after driver install and settings changes affecting Windows tunnel driver selection.
- [x] Preserve existing driver status JSON fields.
- [x] Commit Windows platform readiness optimization.

Verification evidence: `cmake --build --preset windows-release --target win32_driver_status_test exv` and `ctest --test-dir build-windows/cpp -R "win32_driver_status_test|app_api_runtime_policy_test|runtime_status_native_test|service_actions_test" --output-on-failure` passed. The cache stores only the Windows adapter snapshot; `windows_tunnel_driver` and `windows_tap_interface` remain live inputs to `driver_status_json`, so settings changes are not stale even without clearing the adapter snapshot. Driver installation explicitly invalidates the snapshot cache before and after install work.

## Phase 6: UI Locality And Guardrails

- [x] Add frontend test for rapid minimal/advanced toggles while a core RPC is delayed.
- [x] Ensure stale backend/status payload cannot overwrite frontend-local mode.
- [x] Ensure service-install prompt seen state remains frontend-local.
- [x] Ensure Linux host handles `window.setMode` locally.
- [x] Add architecture guard rejecting global desktop dispatch serialization.
- [x] Add timing/log duplication guard for connect timing events.
- [x] Add timing guard for `backend_helper_ready`, `platform_ready`, `protocol_handshake`, `first_failure`, and `serial_tail`.
- [x] Commit UI locality and guardrails.

Verification evidence: `pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts`, `pnpm --dir webui exec vue-tsc -b`, `pnpm --dir webui test:host`, `cmake --build --preset windows-release --target exv app_api_status_contract_test`, and `ctest --test-dir build-windows/cpp -R "app_api_status_contract_test" --output-on-failure` passed. `minimal_mode` and `service_install_prompt_seen` are now frontend-local `localStorage` values; remote settings fetches are overlaid with local values, and saves strip those keys before calling `/config/settings`. Existing host-boundary tests continue to cover local `window.setMode` handling and no synchronous WebView callback core dispatch. New timing source guards require backend/helper, platform, protocol-handshake, first-failure, and serial-tail connect timing marks.

## Phase 7: Verification

- [x] Run focused C++ test set from Task 13.
- [x] Run `pnpm --dir webui test:host`.
- [x] Run `pnpm --dir webui exec vue-tsc -b`.
- [x] Run `pnpm --dir webui run build`.
- [x] Run `./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking`.
- [ ] Manually reproduce: connect, open logs during connect, rapid mode toggles.
- [ ] Confirm logs/status/config do not wait for VPN connect completion.
- [ ] Confirm first-failure branch errors reach the UI before slower branch cleanup finishes.
- [ ] Confirm user cancellation during connect does not trigger an error modal or failure log.
- [ ] Confirm rapid connect/cancel clicking coalesces to the latest user intent.
- [ ] Confirm connect timing shows separate backend/helper, platform, protocol-handshake, and serial-tail branches.
- [ ] Confirm final UI mode equals the last click.
- [ ] Confirm connect failure is visible to the user.
- [x] Confirm `window.setMode` does not appear in core RPC traces.
- [x] Write verification report under `docs/superpowers/reports/`.
- [x] Commit verification report.

Verification report: `docs/superpowers/reports/2026-06-19-core-rpc-lane-isolation-and-connect-pipeline-concurrency-verification.md`. Real VPN connect/cancel/logs reproduction remains unchecked because it requires interactive local VPN operation with a real in-progress connection.

Post-audit correction evidence: `ui_shell_async_host_bridge_test`, `connection_attempt_test`, and `webui/host/__tests__/ui-mode-and-connect-failure.test.ts` are now real runnable acceptance gates. `cmake --build --preset windows-release --target core_rpc_lane_scheduler_test connect_intent_test connect_pipeline_test core_process_lifecycle_test ui_shell_core_rpc_client_test ui_shell_runtime_test ui_shell_async_host_bridge_test app_api_rpc_dispatcher_test core_architecture_contract_test vpn_actions_test connection_attempt_test win32_driver_status_test feedback_test`, `ctest --test-dir build-windows/cpp -R "core_rpc_lane_scheduler_test|connect_intent_test|connect_pipeline_test|core_process_lifecycle_test|ui_shell_core_rpc_client_test|ui_shell_runtime_test|ui_shell_async_host_bridge_test|app_api_rpc_dispatcher_test|core_architecture_contract_test|vpn_actions_test|connection_attempt_test|win32_driver_status_test|feedback_test" --output-on-failure`, `pnpm --dir webui test:host`, `pnpm --dir webui exec vue-tsc -b`, `pnpm --dir webui run build`, and `./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking` passed after the correction. Windows IPC resolver probe correction evidence: `pipe_ipc_test` is now a real release-blocking CTest target; `cmake --build --preset windows-release --target pipe_ipc_test core_process_lifecycle_test core_resolver_test ui_shell_core_rpc_client_test exv-cli`, `ctest --test-dir build-windows/cpp -R "pipe_ipc_test|core_process_lifecycle_test|core_resolver_test|ui_shell_core_rpc_client_test" --output-on-failure`, `./build-windows/cpp/exv-cli.exe status` with no `core_comm_broken` output, `./build-windows/cpp/exv-cli.exe service status` with no `core_comm_broken` output, and `./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking` passed after the correction. Current release-blocking count is 75/75.

2026-06-19 UI responsiveness correction evidence: rapid packaged UI toggle stress reproduced delayed mode bounce before the fix at 8x/12x click bursts, then uncovered a separate Windows `exv-ui` non-responsive state. Root cause was `CoreRpcClient::pump_events()` running from the Win32 UI loop; moving the core event pump to a background thread kept the packaged shell responsive for an 8-second launch probe. `window.setMode` now includes request sequencing, stale native writes are ignored, Win32 resize happens after the renderer RPC response, and the frontend shows a 340 ms resize shield while mode changes are settling. Verification commands passed: `pnpm --dir webui test:host`, `pnpm --dir webui exec vue-tsc -b`, `ctest --test-dir build-windows/cpp -R "win32_webview2_runtime_test|ui_shell_async_host_bridge_test|ui_shell_core_rpc_client_test|core_process_lifecycle_test" --output-on-failure`, `powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop`, and `powershell -ExecutionPolicy Bypass -File scripts\windows-packaging-smoke.ps1`. Manual last-click parity remains unchecked because the available coordinate-based click automation did not reliably hit the WebView toggle after the fix; interactive retest is still required.
