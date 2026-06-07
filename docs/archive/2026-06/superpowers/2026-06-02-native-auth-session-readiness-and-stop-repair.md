# Native Auth-Session Readiness And Stop Repair Plan

## Summary
- Target outcome: native `auth_session` connect becomes a single-outcome flow. It either reaches a stable usable state and only then arms auto-reconnect, or it fails and guarantees that no background supervisor keeps retrying after the UI has already returned an error.
- Root cause:
  - `app_api.cpp` passes `retry_limit=-1` whenever auto-reconnect is enabled.
  - `src/vpn.cpp` previously treated a brief `packet.loop.started` as sufficient readiness.
  - `vpn::start_authenticated` and `helper.handle_start()` could still fail the UI while the durable supervisor kept retrying in the background.
  - The WebUI and tray treated `connected` as the only active-session signal, so `process_running && !connected` became invisible.

## Public And Internal Contract
- Keep desktop RPC paths unchanged.
- Keep the existing `retry_limit` payload field unchanged.
- Extend internal native session state and snapshots with:
  - `packet_loop_started_at_unix_ms`
  - `stable_ready`
- Treat `process_running` as a first-class control signal in the desktop UI.

## L0 Overall Goal
- Make startup and stop behavior coherent across worker, helper, supervisor, and UI.

## L1 Work Packages
- A. Startup readiness contract
  - Replace the implicit `packet.loop.started == ready` rule with a stable-ready gate shared by worker, helper, and supervisor.
- B. Failed-start cleanup
  - Guarantee teardown for every failed native start, including helper service mode.
- C. UI and tray active-session handling
  - Surface `process_running && !connected` as an active, stoppable state.
- D. Diagnostics and regression coverage
  - Add explicit probation / cleanup logging and regression tests for the escaped race.

## L2 Concrete Tasks
### 1. Freeze stable-ready in native session state
- Boundary:
  - `src/vpn_engine/session_state.*`
  - `src/vpn_engine/native_session_store.*`
  - state/store regression tests
- Implementation:
  - Persist `packet_loop_started_at_unix_ms` when `packet.loop.started` fires.
  - Compute `stable_ready=true` only when:
    - the session is still running
    - tunnel metadata is complete
    - the packet loop has remained alive for at least `1500 ms`
- Acceptance:
  - Immediate `packet.loop.started` followed by termination never becomes `stable_ready`.
  - A surviving packet loop becomes `stable_ready` after the dwell window.

### 2. Align worker and supervisor on the same predicate
- Boundary:
  - `src/vpn.cpp`
- Implementation:
  - `vpn::start_authenticated` returns success only after the durable supervisor observes `stable_ready`.
  - `run_native_supervisor_impl()` treats auth-session startup as probationary until `stable_ready`.
  - Before `stable_ready`, a failed first bring-up is terminal and must not reconnect.
- Acceptance:
  - `packet.loop.started -> immediate stop` produces no reconnect attempt.
  - Worker/UI success and supervisor reconnect eligibility cannot diverge.

### 3. Arm reconnect only after stable-ready
- Boundary:
  - `src/vpn.cpp`
- Implementation:
  - Preserve the configured `retry_limit`.
  - Keep reconnect disarmed until the first `stable_ready` observation.
  - Once armed, keep the existing reconnect policy for later retryable failures.
- Acceptance:
  - `auto_reconnect=true` still works after a genuine successful connect.
  - Failed first bring-up never enters background retry.

### 4. Guarantee failed-start teardown in helper mode
- Boundary:
  - `src/helper.cpp`
- Implementation:
  - Run `stop_native_processes_after_failed_start(...)` for any failed native start, not only oneshot mode.
  - Keep cleanup idempotent and safe when only one of PID or supervisor PID exists.
- Acceptance:
  - After `handle_start()` returns a native failure, no lingering supervisor or packet-loop process remains.

### 5. Make UI and tray respect `process_running`
- Boundary:
  - `webui/src/stores/vpn.ts`
  - `webui/src/pages/DashboardPage.vue`
  - `webui/desktop/main/index.ts`
- Implementation:
  - `currentSessionMode` and the dashboard primary action no longer rely on `connected` alone.
  - If `process_running=true && connected=false`, show a destructive stop action instead of a plain connect action.
  - Tray toggle and quit flow disconnect whenever either `connected` or `process_running` is true.
- Acceptance:
  - Background retry is visible and stoppable from both the dashboard and the tray.

### 6. Harden startup-failure diagnostics
- Boundary:
  - `src/vpn.cpp`
  - `src/helper.cpp`
- Implementation:
  - Add explicit stages:
    - `startup_probation_pending`
    - `startup_probation_armed`
    - `startup_probation_failed`
    - `failed_start_cleanup`
  - Prefer persisted native failure codes over the generic fallback error.
- Acceptance:
  - Logs clearly show whether the session failed before stable-ready, when reconnect became armed, and whether failed-start cleanup ran.

### 7. Add regression coverage
- Boundary:
  - `tests/native_session_state_test.cpp`
  - `tests/native_helper_session_test.cpp`
  - targeted runtime / UI verification
- Required scenarios:
  - `packet.loop.started` before dwell window is not stable-ready.
  - Matured packet-loop state becomes stable-ready.
  - Event-recorder persistence preserves the probation-to-stable transition.
  - `process_running && !connected` maps to stop behavior in dashboard / tray logic.
- Acceptance:
  - New tests fail against the old readiness contract and pass once stable-ready semantics are wired through.

## Dependency, Order, And Parallelism
1. Task 1 first
  - Stable-ready is the contract every other fix consumes.
2. Task 2 second
  - Worker and supervisor must agree before cleanup or UI can be trusted.
3. Task 4 after Task 2
  - Cleanup must reflect the final start/failure semantics.
4. Task 5 in parallel with Task 4
  - Once the backend meaning of `process_running` is fixed, UI and tray changes can proceed independently.
5. Task 6 in parallel with Task 4 and Task 5
  - Logging depends on the new state machine, not on UI details.
6. Task 7 spans the whole sequence
  - Add failing regression coverage early, then extend verification after each contract settles.

## Multi-Agent Split
- Agent A: stable-ready state and supervisor arming
  - Owns Tasks 1-3.
- Agent B: helper failed-start teardown
  - Owns Task 4.
- Agent C: dashboard / tray active-session behavior
  - Owns Task 5.
- Agent D: diagnostics and regression harness
  - Owns Tasks 6-7 and validates cross-cutting behavior.

## Verification Plan
- Native tests:
  - `native_session_state_test`
  - `native_helper_session_test`
- Native build checks:
  - `exv`
  - `exv-helper-runtime`
- Frontend / desktop validation:
  - `npm run build` in `webui`
- Manual acceptance:
  - Reproduce the `packet.loop.started -> immediate termination` path and confirm there is no retry after the UI error.
  - Confirm a real successful connect still arms reconnect after stable-ready.
  - Confirm quit / tray stop terminates `process_running && !connected` sessions.

## Assumptions
- Policy: stable-ready is required before auto-reconnect is armed.
- No desktop RPC schema change is needed.
- `stable_ready` stays internal to native session state; the frontend continues to consume `connected` plus `process_running`.
- Existing sync-conflict files remain out of scope unless the active build requires them.
