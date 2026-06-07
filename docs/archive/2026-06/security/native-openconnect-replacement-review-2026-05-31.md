# Native OpenConnect Replacement — Clean-Room & Secret-Handling Review (2026-05-31)

**Task:** H1 — Source / Clean-Room & Secret-Handling Audit
**Plan:** `docs/superpowers/plans/2026-05-31-native-openconnect-extraction-completion-plan.md`
**Scope:** Prove the production native engine is clean-room (not OpenConnect-derived)
and leaks no credentials. Review only — any product fix is routed to P/C/D.

## Method

Static source scans over `src`, `tests`, `docs` (ripgrep), plus targeted reads of
the native protocol/transport/auth modules and the platform TLS backends. Build +
full test suite (25/25) re-run after the one message correction described below.

Note: stray Syncthing artifacts (`*.sync-conflict-*` files under `tests/` and
`src/vpn_engine/protocol/`) appear in raw scan output. They are **not referenced
by CMake**, are **not part of any build target**, and are excluded from this
audit. They must not be committed.

## 1. OpenConnect references — classification

### 1a. Native production engine (`src/vpn_engine/**`)

Only two references exist, both **negative assertions** (the native engine
explicitly rejecting/forgoing OpenConnect):

- `native_engine.cpp:118` — rejects `extra_args`: "Native VPN engine does not
  support legacy OpenConnect extra_args."
- `session_state.hpp:49` — comment: "No OpenConnect process/pid field exists or is
  required."

No native protocol parser, transport, auth, or session-store source references
OpenConnect symbols, data structures, or copied logic. **No production native
source appears OpenConnect-derived.**

### 1b. Legacy fallback engine (gated, diagnostic-only)

All substantive OpenConnect references live in the **legacy fallback** path, used
only when `vpn_engine != "native"`:

- `src/platform/win32/openconnect_process.cpp`,
  `src/platform/darwin/openconnect_process.cpp` — spawn an external OpenConnect
  binary (legacy mode).
- `src/platform/{win32,darwin}/process_control.cpp` — `find_openconnect_pid()`
  (legacy liveness).
- `src/platform/win32/app_api_runtime_policy.cpp` — uses `find_openconnect_pid()`
  for the legacy runtime snapshot.
- `src/platform/win32/service_status.cpp` — legacy runtime-asset diagnostic.
- `src/platform/{win32,darwin}/tunnel_script.cpp`, `openconnect_log.*` — parse
  legacy OpenConnect log evidence (`configure_from_openconnect_log`).

These are classified as **legacy diagnostic fallback**, gated behind non-native
engine selection and the `ECNUVPN_LEGACY_OPENCONNECT_RUNTIME=1` packaging flag.
They are excluded from the native production package (see
`docs/validation/native-packaging-composition-2026-05-31.md`).

### 1c. Placeholder markers (`native_transport_unimplemented` etc.)

`rg "native_transport_unimplemented|native_session_not_durable|fake-server/test-harness"`:

- `native_session_not_durable` — **absent** from `src/` and `tests/` (removed in
  D1; only appears in historical plan text).
- `native_transport_unimplemented` / `native_packet_device_unimplemented` —
  present in `src/vpn_engine/native_engine.cpp` as **defensive dependency-injection
  guards**. They are reachable only when `dependencies_.transport_factory` /
  `packet_device_factory` is null or returns null — i.e. in tests that inject no
  factory. Production `default_native_engine_dependencies()` always injects the
  real win32 Schannel / macOS Secure Transport factory and the platform packet
  device, so these paths are dead in production.
  - **Correction applied** (this audit): the guard messages previously read
    "Native engine production TLS transport is not implemented yet." /
    "...packet device is not available." — stale and misleading now that both are
    implemented. Reworded to "...transport factory is not configured." /
    "...packet device factory is not configured." The error **code constants are
    unchanged** (tests assert on them). Full suite re-run 25/25 after the change.
- `fake-server/test-harness` — not found in production source.

## 2. Secret-handling

- **Events:** the only secret-adjacent emissions in the native engine are
  literal, value-free strings: `native_engine.cpp:291` "password auth started" and
  `:299` "password auth succeeded". No password, cookie, or token **value** is
  ever placed into an event message.
- **Logs:** no `logger::`/`emit_event` call in `src/vpn_engine/**` interpolates a
  password, cookie, secret, or token value (verified by scan).
- **Status/state:** `native-session-state.json` / `SessionState` carry no password
  or cookie field (`session_state.hpp` comment 1b confirms no secret persistence);
  the session store persists interface/ip/pid/phase/failure metadata only.
- The auth cookie jar (`protocol/auth.cpp`) holds the session cookie in memory for
  the CSTP handshake and is cleared on disconnect/reconnect
  (`session.cpp` `cookie_.clear()` at teardown). Passwords are passed to the
  transport in-memory and not serialized.

**Conclusion:** passwords and cookies are confirmed redacted from
events/logs/status.

## 3. TLS verification behavior

- **Windows (Schannel, `src/platform/win32/native_tls_stream.cpp`):**
  `verify_server_certificate()` reads the remote cert via
  `SECPKG_ATTR_REMOTE_CERT_CONTEXT`, builds the chain with
  `CertGetCertificateChain`, and validates it with
  `CertVerifyCertificateChainPolicy` using the **SSL** policy and the SNI host.
  Any failure returns the canonical `tls_verify_failed` code. No verification
  bypass.
- **macOS (Secure Transport, `src/platform/darwin/native_tls_stream.cpp`):**
  `evaluate_server_trust()` installs an `SecPolicyCreateSSL` hostname policy via
  `SecTrustSetPolicies` and evaluates with `SecTrustEvaluateWithError` (falling
  back to `SecTrustEvaluate` + `trust_result_allows_connection` on older SDKs).
  Failure returns `tls_verify_failed`. No verification bypass.

## Verdict

**CLEAN-ROOM PASS.**

- No production native source (`src/vpn_engine/**`, native platform TLS/packet
  backends) appears OpenConnect-derived; the only native-engine OpenConnect
  references are explicit negative assertions.
- All functional OpenConnect references are confined to the gated legacy
  diagnostic fallback engine, excluded from the native production package.
- Passwords/cookies are not emitted to events, logs, or persisted status.
- Server-certificate verification is enforced with hostname-checked platform trust
  APIs on both Windows and macOS.

No clean-room release blocker identified. Live end-to-end validation on real ECNU
infrastructure (F2 Windows manual, G1/G2 macOS) remains ENV-BLOCKED on this host
and is tracked separately.
