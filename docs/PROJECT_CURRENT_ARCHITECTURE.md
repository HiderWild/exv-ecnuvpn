# ECNU-VPN Current Architecture

> Date: 2026-06-14
> Scope: helper/config contract pilot branch

## Runtime Processes

| Process | Privilege | Lifecycle |
|---|---|---|
| `exv` | User | CLI and local control entrypoint |
| `exv-helper --service` | SYSTEM/root | Resident privileged helper daemon |
| `exv-helper --oneshot --endpoint <endpoint> --owner <uid-or-sid> --parent-pid <pid>` | SYSTEM/root | Single Core connection, exits after cleanup |
| Electron desktop | User | UI and desktop RPC client |

There is one helper protocol and one helper handler path. The helper does not
have production legacy worker entries or versioned protocol branches.

## Module Boundaries

`config` owns only configuration reads, writes, field updates, route preference
storage, and credential persistence state. It does not start VPN sessions, call
helper operations, or own runtime tunnel state.

`helper` owns privileged network operations that do not carry credentials:
session lease, tunnel device preparation, tunnel config application, heartbeat,
cleanup, snapshot, and shutdown. It does not receive passwords, cookies, tokens,
or user configuration.

`core` owns VPN authentication, protocol state, reconnect policy, packet/data
plane coordination, and controller state.

## Helper Contract

The helper contract source of truth is `contracts/system.contract.json`; generated
C++ and TypeScript files are checked in and tested for drift.

The helper operation set is:

```text
Hello
StartSession
PrepareTunnelDevice
ApplyTunnelConfig
Heartbeat
Cleanup
GetSnapshot
Shutdown
```

`Hello` must be the first message on a new helper connection. `StartSession`
allows only one active session. `Shutdown` is the active close command: oneshot
cleans and exits, service cleans the session and keeps the daemon alive.

## Lifecycle

Core sends `Heartbeat` immediately after `StartSession`, then every 10 seconds.
The helper maintenance loop runs every 15 seconds and checks heartbeat timeout,
parent process liveness for oneshot, and cleanup work. On timeout, oneshot runs
full cleanup and exits; service runs full cleanup and remains available.
