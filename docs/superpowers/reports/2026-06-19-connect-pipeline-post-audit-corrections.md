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
