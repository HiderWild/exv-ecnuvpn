# Oneshot Helper Leak Correction

Date: 2026-06-19 12:31 +08:00

## Trigger

Manual Phase 7 capture after a reported non-responsive/stuck session found no
active `exv-ui` process, but did find multiple long-lived `exv-helper.exe`
processes. The app log showed repeated one-shot helper startup records without
matching cleanup evidence in the user log.

## Root Cause

`desktop.connect.backend_helper_ready` can start a one-shot helper before the
connect serial tail creates and hands it to `TunnelController`. If another
pipeline branch fails or the connect job is cancelled before the serial tail
takes ownership, the helper endpoint may never receive a first `Hello` request
or client disconnect. That leaves the one-shot helper waiting for a client while
the core process remains alive.

Two post-audit defects were confirmed and fixed:

- `attempt_id` was moved into `TerminalAttemptScope` before the backend branch
  could record the started helper PID.
- If the backend branch completed after another branch had already failed,
  `ConnectPipelineResult.backend` could be empty, so failure-path cleanup had no
  endpoint to consume.

One additional cancellation window was confirmed and fixed:

- If cancellation arrived after `ensure_tunnel_controller(helper_endpoint)` but
  before `controller->set_vpn_config(...)`, the job returned without resetting
  the static controller/client holder.

## Fix

- Added `cleanup_unused_oneshot_backend(...)` in
  `src/core/app_api/desktop_vpn_actions.cpp`.
- The cleanup path is gated to `mode == "oneshot"` plus a non-empty endpoint, so
  service helpers are not touched.
- The cleanup connects to the explicit helper endpoint, sends `Hello`, and
  disconnects. A one-shot helper with no core lease then exits through its
  existing client-disconnect rule.
- The backend branch records helper PID with
  `conn_attempt::update_pids_if_current(...)` before cancellation cleanup.
- Pipeline first-failure, serial-tail pre-controller cancellation, backend
  branch self-cancellation, and post-controller pre-config cancellation now all
  release the unused helper/controller path.

## Verification

RED evidence:

- `ctest --test-dir build-windows/cpp -R "app_api_status_contract_test" --output-on-failure`
  failed after adding the regression guard, first for missing helper PID/update
  and unused one-shot cleanup, then for the post-controller cancellation reset.

GREEN evidence:

- `cmake --build --preset windows-release --target exv app_api_status_contract_test connect_pipeline_test vpn_actions_test connection_attempt_test core_process_lifecycle_test core_architecture_contract_test no_secret_in_logs_test`
- `ctest --test-dir build-windows/cpp -R "core_process_lifecycle_test|core_architecture_contract_test|no_secret_in_logs_test|app_api_status_contract_test|connect_pipeline_test|vpn_actions_test|connection_attempt_test" --output-on-failure`

Spark review:

- First spark pass found the moved `attempt_id` and late-backend-success cleanup
  gaps.
- Second spark pass found the post-controller cancellation cleanup gap.
- Final spark pass reported no remaining real issue in the scoped diff.

## Remaining Scope

This correction prevents new one-shot helper leaks in the covered pipeline
failure/cancellation windows. Existing helper processes from earlier manual
runs are external state and were not killed by this change. The remaining Phase
7 real VPN/UI checks still require interactive reproduction.
