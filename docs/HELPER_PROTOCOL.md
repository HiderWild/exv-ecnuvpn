# Helper Protocol Specification

> Date: 2026-06-14
> Status: Current single helper protocol
> Source of truth: `contracts/system.contract.json`

## Scope

The helper protocol is the privileged local control plane between Core and
`exv-helper`. It is responsible for credential-free network operations only:

- Session lease lifecycle.
- Tunnel device preparation.
- Route, DNS, firewall, and kill-switch application.
- Heartbeat, cleanup, snapshot, and shutdown.

The helper does not authenticate to the VPN server, parse AnyConnect traffic,
store user configuration, or receive secrets.

## Launch Modes

Only these production launches are valid:

```bash
exv-helper --service
exv-helper --oneshot --endpoint <random-endpoint> --owner <uid-or-sid> --parent-pid <pid>
```

`--service` runs as the resident privileged daemon. `--oneshot` runs for one
Core connection and exits after disconnect, shutdown, parent disappearance, or
heartbeat timeout cleanup.

## Transport And Envelope

The transport is a local named pipe on Windows and a Unix domain socket on
macOS/Linux. Messages are newline-delimited JSON using the generated helper
envelope:

```json
{
  "op": 1,
  "payload_json": "{}"
}
```

Responses use:

```json
{
  "op": 1,
  "success": true,
  "error_code": "",
  "error_message": "",
  "payload_json": "{}"
}
```

`Hello` must be the first helper message on a new connection. If the first
message is missing, invalid, or any other operation, the helper rejects it with
`hello_required`. A oneshot helper then performs full cleanup and exits.

## Operations

The only supported operations are:

| Op | Code | Purpose |
|---|---:|---|
| `Hello` | 1 | Report capabilities, launch mode, startup context, and current session state. |
| `StartSession` | 2 | Create the single active helper session. A second active session returns `session_conflict`. |
| `PrepareTunnelDevice` | 3 | Prepare a tunnel adapter/device for the active session. |
| `ApplyTunnelConfig` | 4 | Apply address, route, DNS, firewall, and kill-switch settings. |
| `Heartbeat` | 5 | Refresh the active session lease and report Core phase. |
| `Cleanup` | 6 | Remove resources according to cleanup policy. |
| `GetSnapshot` | 7 | Return helper-visible session and managed-resource state. |
| `Shutdown` | 8 | Cleanup the session; oneshot exits, service stays running. |

`Cleanup` and `Shutdown` are the only close paths. There is no separate
end-session operation.

## Lifecycle

1. Core connects to the helper endpoint.
2. Helper validates the OS peer identity against the launch owner where the
   platform can provide peer credentials.
3. Core sends `Hello` as the first packet within the first-packet timeout.
4. Core sends `StartSession` and receives a non-empty `session_id`.
5. Core sends an immediate `Heartbeat`, then repeats every 10 seconds.
6. Helper maintenance runs every 15 seconds and checks heartbeat timeout,
   parent process liveness, and pending cleanup retry work.
7. Core sends `Cleanup` or `Shutdown` on disconnect.

On heartbeat timeout:

- Oneshot performs full cleanup and exits.
- Service performs full cleanup and keeps the daemon alive.

Cleanup order is route/DNS/firewall first, then adapter/device.

## Security Rules

Helper messages must never contain credential-bearing fields. The manifest
forbidden-field list is enforced by tests and includes password, cookie, token,
secret, auth key, auth token, session cookie, bearer token, and API key names.

Oneshot startup does not use command-line secrets and does not exchange startup
secrets in JSON. Legality comes from a random endpoint, OS peer checks, owner
identity, parent PID liveness, and first-packet `Hello`.
