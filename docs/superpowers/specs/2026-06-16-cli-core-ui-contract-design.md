# CLI, UI, And Core Contract Refactor Design

## Summary

This design makes the desktop UI and CLI two modular frontends over one
core-owned backend. The CLI becomes `exv-cli`, a thin process that parses
terminal arguments, discovers or starts the real core executable, and sends
contracted IPC requests. The UI shell keeps its renderer and host boundary, but
uses the same core resolver, protocol versioning, and lifecycle rules as the
CLI.

The real core executable remains `exv` / `exv.exe`. It owns all business
state: configuration, credentials, VPN lifecycle, helper lease state, retry
policy, route management, logs, service operations, driver/runtime status, and
maintenance actions. UI and CLI do not call core implementation functions
directly.

## Current State

The current repository has partial frontend/backend separation:

- `exv-ui` launches `exv --mode=core` and communicates through the core process
  transport.
- The Vue renderer talks to the native host through generated desktop actions.
- `src/core/pipe_ipc.*` and `src/cli/pipe_client.*` already exist, but normal
  CLI commands do not use them.
- `src/app/main.cpp` still combines regular CLI commands, `--mode=core`,
  `desktop-rpc` compatibility commands, and direct calls into config, VPN,
  service, and log behavior.
- UI and CLI do not have full feature parity: CLI has import/reset/key reset
  and retry semantics that UI does not expose, while UI has routes reset and
  richer settings surfaces.

The target state removes this mixed executable boundary.

## Goals

- Split the real core and CLI into separate executables:
  - `exv` / `exv.exe`: core only.
  - `exv-cli` / `exv-cli.exe`: thin CLI frontend only.
- Keep `exv-ui` as a modular UI frontend.
- Make UI, CLI, and core use one generated communication contract.
- Make CLI startup reuse an existing healthy core when possible.
- Make CLI startup detect broken core communication without automatically
  killing a possibly active session.
- Move CLI-only business semantics into the shared core contract or remove
  them.
- Align CLI configuration keys with the UI configuration model.
- Keep key material opaque to frontends.
- Add safe config import/export, including password-protected export files.
- Add versioned IPC paths, core instance locks, and diagnostic registry files.

## Non-Goals

- Rewriting the Vue renderer.
- Changing the helper privilege boundary beyond the registry cleanup additions
  described here.
- Making `PATH` lookup a security boundary. `PATH` lookup is a convenience
  mechanism; version probing only detects incompatible or mistaken binaries.
- Preserving old unrestricted `config set <key>` behavior.
- Supporting old flat config import files.
- Exposing encryption keys, key paths, or key fingerprints to UI or CLI.

## Process Architecture

```text
exv-ui
  native WebView shell
  shared core resolver
  generated IPC client

exv-cli
  argument parser
  shared core resolver
  generated IPC client
  terminal formatter

exv
  core process
  config, credentials, VPN lifecycle
  helper lease ownership
  logs, routes, service, runtime, drivers
  maintenance actions
```

UI and CLI are frontends. They share the same action names, request schemas, and
response schemas. Core is the only module that mutates persistent config or VPN
runtime state.

## Core Discovery And Startup

UI and CLI use the same resolver:

1. Try the versioned IPC endpoint for the current protocol major version.
2. If the endpoint is usable, send `core.hello`.
3. Reuse the core only if the IPC protocol version is compatible and the
   contract version is accepted.
4. If IPC is unavailable, inspect the versioned core lock.
5. If no live core owns the lock, discover a core executable and start it.
6. If the lock has a live owner but IPC is unavailable, report
   `core_comm_broken` and require explicit user confirmation before terminating
   the residual core.

Core executable discovery order:

1. `EXV_CORE_PATH`, interpreted as a directory. The resolver looks for
   `exv.exe` on Windows and `exv` elsewhere inside that directory.
2. System `PATH`.
3. The current frontend executable directory.

Candidate validation:

- A candidate equal to the current frontend executable is invalid.
- The candidate must exist and be executable.
- The resolver runs `candidate --version`.
- Version output must be uncolored, machine-readable, and match the core
  version pattern.
- A passing version probe means the binary is compatible enough to start. It is
  not a signature or trust proof.

`exv-cli version` and `exv-cli --version` are special: the CLI still discovers
and validates core, then prints the core `--version` output and exits. This
keeps CLI version output aligned with the real core.

## IPC Versioning

IPC endpoint names contain only the IPC protocol major version, such as
`ipc-v1`. Application version and contract version are returned by `core.hello`.

`core.hello` returns:

- `ipc_protocol_version`
- `contract_version`
- `app_version`
- `core_instance_id`
- `pid`
- `core_path`
- `started_at`

This prevents incompatible clients from reusing the wrong core while keeping
minor application upgrades from changing pipe/socket names.

## Core Lock And Registry

Core instance existence is determined by a versioned core lock, not by the
registry and not by the pipe/socket file.

Registry is diagnostic output only. Core owns an in-memory registry snapshot and
periodically writes it to disk using temp-file plus atomic rename. If the
registry file disappears, core recreates it from memory on the next heartbeat.

Registry fields:

- `core_instance_id`
- `pid`
- `core_path`
- `ipc_path`
- `ipc_protocol_version`
- `app_version`
- `contract_version`
- `started_at`
- `last_heartbeat_at`
- `last_known_tunnel_phase`
- `last_known_connected`
- `last_known_network_ready`
- `helper_core_lease_id`

If the registry is corrupt or partially written, readers report
`unknown_state`. They must not treat a missing or corrupt registry as proof that
no core exists.

## Maintenance And Broken Communication

Broken communication states:

- `core_not_running`: pipe unavailable and no live owner holds the lock.
- `core_comm_broken`: pipe unavailable but a live owner holds the lock.
- `core_unresponsive`: pipe connects but `core.hello` or request response
  times out.
- `core_protocol_mismatch`: pipe connects but protocol or contract is
  incompatible.

UI and CLI behavior:

- `core_not_running`: start core.
- `core_comm_broken`: report that core is alive but communication is broken.
- `core_unresponsive`: retry `core.hello` once, then report failure.
- `core_protocol_mismatch`: do not reuse the core; ask user before termination.

If registry indicates `connected` or `network_ready`, the prompt warns that a
VPN session or system network resources may still be active. If the phase is
`connecting`, `reconnecting`, `disconnecting`, or `cleaning_up`, the prompt
warns that an internal flow may still be in progress. Unknown registry state is
treated as unknown risk.

Cleanup requires explicit confirmation:

- UI displays a confirmation dialog.
- Interactive CLI prompts `[y/N]`.
- Non-interactive CLI fails by default unless the user passes an explicit
  cleanup flag such as `--kill-stale-core`.
- The IPC request carries `confirm: true`.

Termination flow:

1. Prefer a graceful maintenance shutdown if communication is possible.
2. If communication is broken, terminate by PID after confirmation.
3. POSIX sends `SIGTERM`, waits, then escalates only under force policy.
4. Windows uses process termination when no IPC shutdown path is available.
5. Verify PID dead, lock released, and IPC endpoint unavailable before starting
   a new core.

## Helper Timeout Registry Cleanup

Helper core lease timeout cleanup also handles core registry cleanup.

Rules:

- Helper first performs existing active session cleanup.
- Helper deletes a registry only after cleanup succeeds.
- Deletion is compare-and-delete. The registry must still match:
  - `core_instance_id`
  - `pid`
  - `helper_core_lease_id`
  - `ipc_protocol_version`
- Partial cleanup failure preserves registry for diagnostics.
- Helper may only touch registry files under the canonical state directory and
  with versioned registry names.
- Old helpers must not be able to delete a new core registry after a restart.

## Unified Contract Actions

The generated system contract becomes the only source of frontend actions.

Core actions include:

- `core.hello`
- `status.get`
- `vpn.connect`
- `vpn.disconnect`
- `config.get`
- `config.saveAuth`
- `config.saveSettings`
- `config.reset`
- `config.import`
- `config.export`
- `key.status`
- `key.reset`
- `routes.list`
- `routes.add`
- `routes.remove`
- `routes.reset`
- `logs.list`
- `service.status`
- `service.install`
- `service.uninstall`
- `runtime.status`
- `drivers.status`
- `drivers.install`
- `maintenance.inspectCore`
- `maintenance.killStaleCore`

Destructive actions require `confirm: true`:

- `config.reset`
- `key.reset`
- `maintenance.killStaleCore`

The core may return `confirmation_required` when the field is missing or false.

## Configuration Model

The shared config model follows the UI shape. CLI no longer exposes arbitrary
advanced key writes.

`auth` fields:

- `server`
- `username`
- `remember_password`
- `password_stored`
- `user_agent`

`settings` fields:

- `mtu`
- `dtls`
- `extra_args`
- `log_path`
- `vpn_engine`
- `openconnect_runtime`
- `windows_tunnel_driver`
- `windows_tap_interface`
- `auto_reconnect`
- `retry_limit`
- `minimal_mode`
- `service_install_prompt_seen`
- `minimal_install_service_before_connect`

`routes` supports list, add, remove, and reset.

`key` exposes only coarse management state:

- `available`
- `present`
- `status`

No frontend response includes key material, key path, or fingerprint.

## Retry Policy

`retry_limit` replaces CLI one-shot retry parameters:

- `-1`: unlimited retries.
- `0`: no retry.
- `N > 0`: retry at most N times after the initial failed attempt.

`exv-cli start -rt [count]` writes `settings.retry_limit`. It does not pass a
retry value as part of `vpn.connect`.

When `vpn.connect` starts, core reads persistent config, captures
`retry_limit`, and stores it in the active session snapshot. Later config saves
or resets do not change the active session retry policy. A future live policy
change must use an explicit VPN action rather than implicit config mutation.

## Config Reset And Key Reset

`config.reset` resets persistent config only:

- It does not disconnect the active VPN session.
- It does not clean helper leases, routes, DNS, adapters, or firewall state.
- It does not mutate values already captured by the current session.
- It affects the next connection and config screens.

`key.reset` manages saved credentials only:

- It resets or creates the local key.
- It clears the persisted encrypted password.
- It does not disconnect the active VPN session.
- It does not expose key content or key file paths.
- A current session may keep using the already supplied password until that
  session ends.
- The next connection needs a newly provided password.

If either action is invoked while a session is active, UI and CLI warn that the
current connection will not be interrupted and the effect applies to later
connections or saved credentials.

`status.get` must separate active session state from persisted config state.
When a session is active, displayed connection server, username, route count,
MTU, and retry policy come from the active session snapshot, not from the
current config file.

## Config Import And Export

`config.import` accepts only the new contract format. Old flat JSON files are
not supported.

`config.export` has two modes:

- Non-sensitive export: no VPN password is included.
- Password-protected export: visible config fields remain readable, and the
  VPN password is encrypted.

Password-protected export uses libsodium:

- Argon2id derives export keys from the user-provided export password.
- A keyed MAC protects the canonical config body.
- XChaCha20-Poly1305 AEAD encrypts the VPN password.
- The AEAD additional authenticated data is the canonical config body, so
  visible fields such as server and username cannot be silently changed while
  reusing the same password ciphertext.

The export file stores:

- `format_version`
- visible config body
- `protection.kdf`: algorithm, salt, and parameters
- `protection.mac`: keyed MAC over the canonical body
- `protection.password`: AEAD algorithm, nonce, and ciphertext/tag

The export file never stores local key material.

Import with password protection:

1. Parse the file.
2. Ask for the export password.
3. Derive export keys using the stored KDF parameters.
4. Verify the canonical body MAC.
5. If verification fails, reject with "password is wrong or the file was
   modified".
6. Decrypt the VPN password with AEAD.
7. If no local key exists, ask whether to create one. Cancellation aborts the
   import.
8. Re-encrypt the VPN password using the current local key.
9. Persist the imported config transactionally.

No `EXV` plaintext probe is used. AEAD and MAC failures are not over-classified
because wrong password, tampering, and protection-data corruption can share the
same observable failure surface.

The implementation should use secure buffers and explicit zeroization for
temporary plaintext passwords. Plaintext should be passed directly to the
consumer that encrypts it with the current local key or starts a VPN session.
It should not be copied through multiple intermediate strings.

## User Warnings For Sensitive Export

After password-protected export, UI and CLI display a high-risk warning:

```text
The exported configuration contains a recoverable VPN password. Treat it as a
sensitive file. Do not share it through untrusted channels or copy it to public
machines. The password is not stored in plaintext, but a weak export password
can still be attacked offline. Destroy the file after use when possible.
```

The UI presents this as a destructive/sensitive action warning. CLI prints it
with high-severity styling.

## CLI Surface

CLI commands are thin mappings:

- `exv-cli start`: `vpn.connect`
- `exv-cli start -rt [count]`: `config.saveSettings` for `retry_limit`, then
  `vpn.connect`
- `exv-cli stop`: `vpn.disconnect`
- `exv-cli status`: `status.get`
- `exv-cli config show`: `config.get`
- `exv-cli config import <file>`: `config.import`
- `exv-cli config export <file>`: `config.export`
- `exv-cli config reset`: `config.reset` with confirmation
- `exv-cli config set <allowed-key> <value>`: maps only to allowed auth or
  settings fields
- `exv-cli config key status`: `key.status`
- `exv-cli config key reset`: `key.reset` with confirmation
- `exv-cli config routes list/add/remove/reset`: route actions
- `exv-cli service status/install/uninstall`: service actions
- `exv-cli logs`: `logs.list`
- `exv-cli version`: core `--version` passthrough

Legacy `desktop-rpc` entrypoints are not part of the user CLI surface. They may
exist temporarily as migration shims but should not be the architecture used by
UI or CLI.

## UI Surface

UI keeps its current settings, routes, service, drivers, runtime, logs, and VPN
lifecycle features. It adds:

- config import
- config export with and without password
- config reset
- key reset
- retry limit setting
- core maintenance prompt for broken communication

Key UI rules:

- Key content is never displayed.
- Key path and fingerprint are not displayed.
- Password-protected export warns after success.
- Active-session warnings are shown for config reset and key reset.
- Broken core prompts explain the possible impact before termination.

## Error Handling

Standard errors:

- `confirmation_required`
- `invalid_payload`
- `invalid_config`
- `unsupported_contract_version`
- `core_comm_broken`
- `core_unresponsive`
- `core_protocol_mismatch`
- `core_not_found`
- `core_launch_failed`
- `core_version_probe_failed`
- `config_import_format_unsupported`
- `config_import_auth_failed`
- `config_import_tampered_or_wrong_password`
- `credential_store_unavailable`
- `key_missing`
- `key_corrupt`

All failures return structured codes and human messages. UI and CLI format the
same core responses differently but do not invent separate business semantics.

## Testing Strategy

Contract tests:

- Generated C++ and TypeScript action constants match the manifest.
- UI and CLI action allowlists match the generated contract.
- Destructive actions require `confirm: true`.

CLI thin-layer tests:

- CLI commands serialize the expected IPC request.
- CLI never calls config, VPN, service, route, or log business functions
  directly.
- `exv-cli version` performs discovery and version passthrough.
- `-rt` writes `settings.retry_limit` and then calls `vpn.connect`.

Core resolver tests:

- `EXV_CORE_PATH` is treated as a directory.
- `PATH` lookup and same-directory lookup work.
- Current frontend executable is rejected as a core candidate.
- Version probe accepts only the machine-readable core version pattern.
- Live pipe is reused.
- Live lock with broken pipe reports `core_comm_broken`.

Core lifecycle tests:

- Core writes and recreates registry from in-memory snapshot.
- Registry writes are atomic.
- Core lock prevents duplicate cores when socket files disappear.
- `core.hello` returns version and instance data.

Maintenance tests:

- UI/CLI do not kill stale core without confirmation.
- Non-interactive CLI fails unless explicit cleanup flag is present.
- Helper compare-and-delete cannot remove a new core registry.
- Partial helper cleanup preserves registry.

Config tests:

- Import rejects old flat config format.
- Config reset does not alter active session snapshot.
- Key reset clears persisted password and does not disconnect active session.
- Status separates active session and persisted config.
- Retry policy is captured at connect time.

Crypto tests:

- Non-sensitive export excludes password protection fields.
- Password-protected export verifies MAC and decrypts password with the correct
  export password.
- Wrong password or modified body rejects import.
- Modified visible server or username fails password import because AEAD AAD no
  longer matches.
- Local key is not written until protected import has validated and user has
  confirmed key creation.
- Temporary plaintext buffers are zeroized in supported code paths.

Frontend tests:

- UI exposes import/export/reset/key reset/retry limit.
- UI warning text appears for active-session reset and sensitive export.
- UI maintenance prompt appears for `core_comm_broken`.

## Migration Plan

1. Update the contract manifest with core, config, key, maintenance, and export
   actions.
2. Add core hello, versioned IPC path, core lock, and registry.
3. Move config reset, key reset, retry limit, import, and export into core
   use cases.
4. Add libsodium-backed export protection.
5. Add shared core resolver used by UI and CLI.
6. Split `exv-cli` from `exv`.
7. Convert CLI commands to IPC-only calls.
8. Add UI controls for missing CLI-only semantics.
9. Retire direct CLI business calls and legacy desktop-rpc user surface.

Each step should leave tests passing and preserve the ability to start core and
query `status.get`.

## Accepted Decisions

- Core and CLI are separate executables.
- IPC path carries protocol major version only.
- `core.hello` reports app and contract versions.
- `EXV_CORE_PATH` is a directory.
- Core candidates are validated with `--version`.
- `PATH` lookup remains a convenience lookup.
- Broken communication with a live core requires user confirmation before
  termination.
- Registry is diagnostic output, not a lock.
- Helper may clean matching registry only after successful timeout cleanup.
- Config import accepts only the new contract format.
- Config reset and key reset do not disconnect the active session.
- Key state is opaque to frontends.
- `retry_limit` is persistent config read by core at connect time.
- Password-protected export uses password-derived MAC plus AEAD, not encrypted
  checksum or fixed plaintext probes.
