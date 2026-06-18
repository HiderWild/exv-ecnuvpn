# Core RPC Lane Isolation And Connect Pipeline Concurrency Verification

Date: 2026-06-19 04:51 +08:00

## Automated Verification

Passed:

- `ctest --test-dir build-windows/cpp -R "app_api_status_contract_test|core_process_lifecycle_test|connect_intent_test|connect_pipeline_test|vpn_actions_test|core_session_runner_test|native_engine_contract_test|native_handshake_job_test|win32_driver_status_test|app_api_runtime_policy_test" --output-on-failure`
- `pnpm --dir webui test:host`
- `pnpm --dir webui exec vue-tsc -b`
- `pnpm --dir webui run build`
- `./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking`

Release-blocking result: 72/72 tests passed.

## Verified Behaviors

- Desktop `vpn.connect` returns accepted job state promptly while heavy work runs in the background.
- Core RPC lane scheduling keeps read/log/config work isolated from VPN control work.
- Desktop connect now runs backend/helper readiness, platform readiness, and native protocol handshake as pipeline branches before entering the controller serial tail.
- Route/DNS/adapter mutation remains in the controller network-config serial tail after readiness branches succeed.
- Native auth/CSTP handshake can be prepared once and adopted by `CoreSessionRunner` without re-authentication or a second CSTP connect.
- Windows driver readiness uses one cached adapter snapshot for burst status/preflight calls instead of separate Wintun and TAP adapter scans.
- `minimal_mode` and `service_install_prompt_seen` are frontend-local values backed by `localStorage`; remote settings payloads cannot overwrite them.
- Connect timing source guards cover backend/helper, platform, protocol handshake, first failure, and serial tail markers.

## Manual Verification Not Run

The following require interactive local VPN/UI operation and were not executed by this agent:

- Real connect against the VPN gateway.
- Open logs while a real VPN connect is in progress.
- Rapidly toggle minimal/advanced mode in the packaged UI and visually confirm the final mode equals the last click.
- Confirm first-failure branch errors reach the visible UI before slower branch cleanup finishes.
- Confirm user cancellation during a real connect does not show an error modal or failure log.
- Confirm real connect timing output in application logs.
