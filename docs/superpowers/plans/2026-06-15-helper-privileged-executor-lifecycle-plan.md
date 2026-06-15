# Helper Privileged Executor Lifecycle Plan

## Summary
Helper is now the core-process-scoped privileged executor, not a VPN-session-scoped temporary cleanup process. A one-shot helper is started only by controlled core bootstrap when privileged work is needed. After elevation succeeds, it stays alive for the lifetime of the owning core process and exits only when core releases its lease or core heartbeat times out.

The persistent service helper exposes the same privileged operation contract. Its difference is lifecycle management: the OS service manager owns startup, restart, and boot persistence.

## Key Changes
- Add a `CoreLease` that is independent of VPN sessions.
- Require `Hello` first, then `AcquireCoreLease`, before privileged helper operations.
- Keep core lease alive with `KeepAlive`; retain existing VPN/session heartbeat for active tunnel cleanup.
- Make VPN `Shutdown` clean only the VPN session; it must not make a one-shot helper exit while the core lease is active.
- Add `Inspect` so core/desktop can query the current helper instance mode, lease, session, and queue state.
- Move all privileged mutations behind helper: service install/uninstall, adapter create/delete, route add/remove, DNS apply/restore.
- Introduce a serialized cleanup lease owned by helper as the record of actual platform rollback facts.

## Phases
1. Contract and status: manifest, generated artifacts, helper messages/client methods, `Hello/Inspect` runtime state.
2. Core lease lifecycle: one-shot long residency, release path, timeout cleanup and exit.
3. Privileged task queue: IPC parses requests, runtime enqueues tasks, one worker serializes privileged mutations.
4. Service maintenance: helper-owned install/uninstall and core orchestration.
5. Cleanup lease and handoff: one-shot exports actual cleanup facts, service imports/adopts them, core switches endpoint.
6. Platform completion: Windows full rollback facts, macOS DNS policy, Linux `PlatformNetworkOps`.

## Acceptance
- One-shot helper remains idle after VPN disconnect while core lease is alive.
- One-shot helper cleans all active resources and exits when core lease times out.
- Service helper cleans active resources on core lease timeout but keeps daemon running.
- `helper.status` can distinguish current helper instance mode from installed service status.
- Service install can run while VPN is connected and hand off cleanup ownership to service.
- Service uninstall refuses active VPN until cleanup succeeds.

## Current Progress
As of 2026-06-15, Phase 1 through Phase 6 are implemented at the code and
contract-test level in the working tree. Real elevated OS acceptance is still
required on each platform before release:
- Manifest and generated C++/TypeScript artifacts include helper status,
  core lease, task queue, service maintenance, cleanup lease, and handoff
  messages.
- Helper runtime enforces `Hello`/`AcquireCoreLease` before privileged
  operations, binds leases to verified peer identity where available, and keeps
  `Inspect`/`Hello` state redacted for unauthorized peers.
- One-shot `Shutdown` now cleans only the VPN session; the helper remains alive
  while the core lease is active. `ReleaseCoreLease(exit_if_oneshot=true)` and
  core lease heartbeat timeout are the normal/abnormal one-shot exit paths.
- `PrivilegedTaskQueue` serializes network and service mutations, exposes queue
  state, does not keep handler locks across platform work, and unblocks pending
  callers during shutdown.
- Service install/uninstall are helper-owned privileged operations. Desktop and
  native core RPC route through helper IPC, and uninstall rejects active VPN
  sessions with `vpn_session_active`.
- CLI `exv service install/uninstall` now dispatches through core/app service
  actions instead of calling platform service managers directly. The public
  `helper::install_service` and `helper::uninstall_service` wrappers were
  removed, leaving the platform service manager reachable only from helper
  service operations.
- Electron main no longer exposes the legacy elevated `serviceCommand` IPC
  fallback or platform `runServiceCommandElevated` implementations. Renderer
  service install/uninstall uses generated core RPC service actions.
- First service install can explicitly bootstrap a one-shot helper, send
  `Hello`, acquire a core lease, invoke helper `InstallService`, and release the
  lease after the one-off maintenance task.
- Connected one-shot service install performs the handoff sequence:
  `InstallService -> ExportCleanupLease -> service Hello/AcquireCoreLease ->
  HandoffSession -> TunnelController helper replacement ->
  FinalizeHandoff(exit=true)`.
- `CleanupRecord::managed_resources` is now first-class and persisted directly;
  the old `__managed__` firewall-rule side channel has been removed.
- `PlatformNetworkOps` has a resource-aware cleanup entry point and a
  `managed_resources()` fact export hook so a fresh service helper can consume
  imported cleanup facts instead of relying only on in-memory platform objects.
- Windows exports adapter/LUID/ifIndex, exact native route rows, and original
  DNS settings as platform facts; focused tests verify that a fresh backend can
  remove routes, restore DNS, and delete Wintun using only imported facts.
- macOS now has code-level DNS apply/restore through a Darwin DNS API seam,
  exports adapter/route/DNS facts, and can clean imported facts from a fresh
  backend in focused tests.
- Linux now has a native `PlatformNetworkOps` factory path backed by
  `/dev/net/tun`, `ip`, and `resolvectl`, with persistent tun devices and
  structured adapter/interface/route/server-bypass/DNS facts for imported
  cleanup.

## Remaining OS Acceptance Work
The code now has the contract, orchestration, and platform fact model for the
full helper executor design. Release still requires OS-level acceptance:
- Windows has focused fake-API coverage for durable rollback facts, but still
  needs elevated local acceptance against real Wintun/IP Helper/DNS APIs.
- macOS has focused fake-API coverage for route/DNS/adapter fact cleanup, but
  needs elevated local acceptance against real utun, route socket, and scutil
  DNS behavior.
- Linux source/model checks and a WSL syntax check cover the native backend in
  this session, but privileged Linux acceptance must still cover tun create,
  route/DNS apply, cleanup, and one-shot-to-service handoff.
