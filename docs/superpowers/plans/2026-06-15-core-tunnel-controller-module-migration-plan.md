# Core Tunnel Controller Staged Module Migration Plan

Date: 2026-06-15

## Summary

`core/tunnel_controller` is the next high-value module boundary after helper
and config. It owns connection lifecycle, reconnect policy, heartbeat
coordination, event handling, and status snapshots consumed by App API, core
process, and RPC actions. The migration is intentionally staged so each phase
can be implemented, verified, and handed off independently.

This plan does not change connect, disconnect, reconnect, heartbeat, cleanup,
helper session, or native engine behavior.

## Phases

1. Contract kernel and boundary gates:
   - Add `modules.tunnel_controller` to the system contract manifest.
   - Generate tunnel phase, event, disconnect reason, and error-domain
     constants into C++ and TypeScript artifacts.
   - Add tests that compare generated tunnel contracts with the existing C++
     public enums.
   - Add architecture checks for controller/platform/helper dependency
     boundaries.

2. Public state contract module:
   - Add `exv.core.tunnel.contract`.
   - Export only simple read-only contract helpers for phases, events,
     disconnect reasons, and error domains.
   - Add an import smoke test that compares the module with generated
     contract artifacts.
   - Keep JSON/status serialization in ordinary C++ for this phase.

3. Runtime implementation split:
   - Replace `tunnel_controller_*.inc.cpp` with normal private implementation
     units.
   - Keep `TunnelController` as a PIMPL public facade.
   - Move shared state into a private implementation header used only inside
     `src/core/tunnel_controller`.
   - Convert mirror state-machine tests to exercise the real controller.

4. Internal component modules:
   - Migrate low-coupling internals behind modules such as reconnect, timing,
     error mapping, and events.
   - Keep compatibility headers during the transition.
   - Convert focused tests for those internals to import modules first.

5. Public controller module:
   - Add `exv.core.tunnel` as the public controller module.
   - Export the public controller API and status/intent/event types.
   - Keep implementation details and platform/helper/native engine
     dependencies outside the exported interface.

6. Compatibility cleanup:
   - Remove or clearly mark legacy include shims.
   - Enforce no duplicate phase-to-string switches.
   - Enforce no controller private-header imports outside the controller
     implementation.

## Verification

Focused verification for Phase 1:

```powershell
cmake --build build --target contract_manifest_test contract_generation_check tunnel_contract_test app_api_status_contract_test vpn_actions_test core_process_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(contract_manifest_test|contract_generation_check|tunnel_contract_test|app_api_status_contract_test|vpn_actions_test|core_process_test)$"
```

Final verification after each completed phase:

```powershell
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure
git diff --check
```

## Notes For Future Agents

The current source of truth for this lane is this staged plan plus the matching
design document in `docs/superpowers/specs`. Do not jump directly to full
controller module export before the contract and runtime split phases are
complete.
