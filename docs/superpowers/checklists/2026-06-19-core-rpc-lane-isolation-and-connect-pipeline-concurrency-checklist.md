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

- [ ] Add `CoreRpcClient::invoke_async()`.
- [ ] Add id/request_id pending map.
- [ ] Add one reader pump that demultiplexes responses and events.
- [ ] Preserve blocking `invoke()` as wrapper for compatibility.
- [ ] Resolve all pending requests on transport close.
- [ ] Add `AsyncHostBridge`.
- [ ] Keep `window.setMode` and `window.resolveClosePrompt` local to shell.
- [ ] Ensure host message acceptance does not wait for core.
- [ ] Wire Windows WebView2 posting callback.
- [ ] Wire macOS WKWebView main-thread posting callback.
- [ ] Wire Linux WebKitGTK main-thread posting callback.
- [ ] Commit host/client async bridge.

## Phase 4: Parallel VPN Connect Pipeline

- [ ] Add accepted-job tests for `vpn.connect`.
- [ ] Add busy-connect coalescing test with `accepted=true`, `coalesced=true`, and `active_job_id`.
- [ ] Add `connect_intent_test` for latest user intent and epoch reconciliation.
- [ ] Add user-cancel-during-connect test where `vpn.disconnect` returns `accepted=true`, `cancelling=true`, and `user_cancelled=true`.
- [ ] Prove user cancellation does not set `last_error` and does not show an error modal.
- [ ] Prove connect/cancel/connect/cancel while busy results in one active workflow and latest desired intent `Disconnect`.
- [ ] Prove connect/cancel/connect while busy starts exactly one new connect after cleanup reaches idle.
- [ ] Prove a non-user connection failure does not retry unless a later user click increments intent epoch.
- [ ] Add disconnect-during-connect responsiveness test.
- [ ] Add `connect_pipeline_test` for branch concurrency.
- [ ] Prove backend/helper, platform readiness, and protocol handshake branches run concurrently.
- [ ] Prove first branch failure returns to UI without waiting for slower branches.
- [ ] Prove cancellation is requested for losing branches after first failure.
- [ ] Prove late non-cancel branch failures are logged with job id and discarded reason.
- [ ] Prove successful late branch results are discarded silently after first failure.
- [ ] Prove serial network tail starts only after all three branches succeed.
- [ ] Create `VpnConnectJobOwner`.
- [ ] Create `connect_intent.hpp/.cpp`.
- [ ] Create `ConnectPipeline`.
- [ ] Create `NativeHandshakeJob`.
- [ ] Split native auth/CSTP handshake from packet-device and network-config attach.
- [ ] Move helper/backend readiness out of the RPC request handler.
- [ ] Move platform/runtime readiness out of the RPC request handler.
- [ ] Return `accepted=true`, `phase=connecting`, and `job_id` promptly.
- [ ] Keep route/DNS/adapter mutation in the serial tail after all readiness branches pass.
- [ ] Keep `vpn.disconnect` responsive by requesting cancellation.
- [ ] Wire the yellow in-progress button to `cancelConnect()`.
- [ ] Make `cancelConnect()` send `vpn.disconnect` while `connectInFlight` is true.
- [ ] Make UI switch immediately to the disconnected/cancelled visual state on user cancellation.
- [ ] Update frontend connect state to rely on status/events after accepted response.
- [ ] Commit parallel connect pipeline conversion.

## Phase 5: Windows Platform Readiness

- [ ] Add Windows driver status tests with injected adapter enumeration.
- [ ] Prove preflight does not run separate Wintun and TAP enumeration when one scan is enough.
- [ ] Replace double PowerShell/CIM scans with one native or single-command adapter snapshot.
- [ ] Add short-lived driver status cache for connect/status bursts.
- [ ] Invalidate driver cache after driver install and settings changes affecting Windows tunnel driver selection.
- [ ] Preserve existing driver status JSON fields.
- [ ] Commit Windows platform readiness optimization.

## Phase 6: UI Locality And Guardrails

- [ ] Add frontend test for rapid minimal/advanced toggles while a core RPC is delayed.
- [ ] Ensure stale backend/status payload cannot overwrite frontend-local mode.
- [ ] Ensure service-install prompt seen state remains frontend-local.
- [ ] Ensure Linux host handles `window.setMode` locally.
- [ ] Add architecture guard rejecting global desktop dispatch serialization.
- [ ] Add timing/log duplication guard for connect timing events.
- [ ] Add timing guard for `backend_helper_ready`, `platform_ready`, `protocol_handshake`, `first_failure`, and `serial_tail`.
- [ ] Commit UI locality and guardrails.

## Phase 7: Verification

- [ ] Run focused C++ test set from Task 13.
- [ ] Run `pnpm --dir webui test:host`.
- [ ] Run `pnpm --dir webui exec vue-tsc -b`.
- [ ] Run `pnpm --dir webui run build`.
- [ ] Run `./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking`.
- [ ] Manually reproduce: connect, open logs during connect, rapid mode toggles.
- [ ] Confirm logs/status/config do not wait for VPN connect completion.
- [ ] Confirm first-failure branch errors reach the UI before slower branch cleanup finishes.
- [ ] Confirm user cancellation during connect does not trigger an error modal or failure log.
- [ ] Confirm rapid connect/cancel clicking coalesces to the latest user intent.
- [ ] Confirm connect timing shows separate backend/helper, platform, protocol-handshake, and serial-tail branches.
- [ ] Confirm final UI mode equals the last click.
- [ ] Confirm connect failure is visible to the user.
- [ ] Confirm `window.setMode` does not appear in core RPC traces.
- [ ] Write verification report under `docs/superpowers/reports/`.
- [ ] Commit verification report.
