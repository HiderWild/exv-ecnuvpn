# Bidirectional Data-Plane Forwarding — 2026-05-31 (Task P2)

Plan: `docs/superpowers/plans/2026-05-31-native-openconnect-extraction-completion-plan.md`

## Scope

Replace the previous 1:1 request/response coupling (`exchange_packet`) with a
continuous, full-duplex forwarding loop that moves traffic between the local
packet device (utun/wintun) and the CSTP-over-TLS transport in both directions
independently, with deterministic, race-free shutdown.

This is the mock-level acceptance for P2. Live concurrent-TLS validation against
a real ECNU AnyConnect peer is **ENV-BLOCKED** on this host (no live peer / no
credentials); the design notes below record how production correctness is
preserved so the live path is a configuration concern, not a redesign.

## Transport interface change

`ProtocolTransport` (`src/vpn_engine/protocol/session.hpp`) dropped the coupled
`exchange_packet` and now exposes direction-independent primitives:

| Method | Direction | Blocking |
|---|---|---|
| `send_packet(payload)` | outbound (device → peer) | non-blocking write |
| `send_control(InboundFrameKind)` | outbound control (DPD/keepalive/disconnect) | non-blocking write |
| `receive_frame(InboundFrame*)` | inbound (peer → device) | **blocking** read |

`InboundFrame { InboundFrameKind kind; std::vector<uint8_t> payload; }` where
`kind ∈ { none, data, dpd_request, dpd_response, keepalive, disconnect }`.

## Two-thread full-duplex model

`ProtocolSession::run_forwarding` (`src/vpn_engine/protocol/session.cpp`) runs
two threads:

- **Inbound thread** — `while (true)` loop calling `transport_->receive_frame`
  (blocking). `data` frames are written to the packet device and emit
  `packet.inbound`; `disconnect` records a fatal `transport_closed` reason;
  `dpd_request`/`dpd_response`/`keepalive`/`none` are currently ignored (the P3
  hook point). The loop is **not** gated on the stop flag so frames already
  queued when the outbound side ends are drained before `receive_frame` reports
  closure. On exit it only sets `stop = true`; it **never** closes the device.
- **Outbound thread** — the caller/loop thread polls the **non-blocking** packet
  device (`read_packet` returns `no_data`/`would_block`/`packet_device_empty`),
  sleeping 1 ms between empty polls up to `packet_loop_no_data_poll_limit`
  (default 1000). Readable packets are sent with `transport_->send_packet` and
  emit `packet.outbound`.

### Device-vs-transport blocking asymmetry (critical design fact)

The packet **device** read is non-blocking and is **polled**; the TLS
**transport** read (`receive_frame`) is **blocking**. Consequences:

- The device is closed **only by the loop/outbound thread**, preserving the
  "close on the same thread that reads" invariant the engine contract test
  asserts. The outbound thread can self-terminate (it observes `stop`/cancel
  between polls) and then close the device.
- To unblock the inbound thread's blocking `receive_frame`, the outbound thread
  calls `transport_->disconnect()` after its loop, then `inbound.join()`.

## Termination state machine

`run_forwarding` records the first termination reason under `std::mutex
reason_mu` via `set_reason(new_reason, result)`:

| Reason | Meaning |
|---|---|
| 0 | none |
| 1 | graceful (clean packet-loop end) |
| 2 | cancelled |
| 3 | fatal (e.g. peer `disconnect` / transport error) |

Only the **first** reason is kept, and `set_reason` always sets the atomic
`stop` flag. This prevents the expected post-disconnect `transport_closed` the
inbound thread sees from overwriting an earlier graceful/cancelled stop.

Graceful end closes the device but does **not** call `state_.stopped()`, leaving
the session network-ready (matches prior behavior). Cancellation routes through
`stop_cancelled`. A fatal `transport_closed` with `auto_reconnect` and remaining
attempts triggers `reconnect()`.

## TLS concurrency hardening (win32 Schannel)

With independent reader/writer threads sharing one TLS stream,
`src/platform/win32/native_tls_stream.cpp` was hardened:

- `production_transport` serializes writes with `write_mutex_`; `disconnect()`
  and `reset_for_reconnect()` also take it.
- `NativeTlsStream` adds `io_mutex_` and an atomic `closed_`. The decrypt path
  and `encrypted_buffer_` mutation run under `io_mutex_`, so `close()` (driven
  from the outbound thread) can never destroy the Schannel context while the
  reader is mid-`DecryptMessage`. The **blocking `recv()` stays outside the
  lock**; `close()` interrupts it by closing the socket, after which the reader
  re-checks `closed_` under the lock and bails before reusing the torn-down
  context. Schannel permits one thread encrypting while another decrypts on the
  same context, so the writer does not take `io_mutex_` across its blocking
  `send()`.

## Verification

Mock-stream / mock-device suites prove the acceptance criteria:

- `native_protocol_session_test` — multi-frame inbound drain, multi-packet
  outbound, graceful network-ready, cancel-safe shutdown, reconnect device
  close accounting.
- `native_engine_contract_test` — engine-level full-duplex with thread-safe
  blocking-queue transport and polling devices; close-on-read-thread invariant.
- `native_production_transport_test` — `send_packet` data-frame encode +
  partial inbound `receive_frame`; EOF mid-stream → `transport_closed`.
- `native_fake_anyconnect_server_test` — thread-safe echo transport round-trip.
- `native_tls_stream_contract_test` + `win32_native_tls_stream_test` — TLS
  stream functional contract after the concurrency refactor.

Command: `ctest --preset windows-release --output-on-failure` → **25/25 pass**.

Also fixed a pre-existing CMake link gap: `native_event_sink_test` linked
`native_engine.cpp` without its protocol/platform sources, surfacing on a full
build as an undefined `ProductionProtocolTransport` ctor; its source list now
mirrors `native_engine_contract_test`.

## ENV-BLOCKED

- **P2-live-concurrent-TLS** — concurrent inbound/outbound forwarding against a
  real AnyConnect/CSTP peer with live Schannel encrypt+decrypt under load. No
  live peer or credentials on this Windows host. The mock suites above are the
  plan's acceptance for P2; the live path is gated on environment access.

---

# Liveness Servicing & Reconnect — 2026-05-31 (Task P3)

Builds on the P2 forwarding loop to add DPD/keepalive servicing and dead-peer
driven reconnect. All timers are expressed in **consecutive idle outbound-poll
counts** (one idle poll ≈ 1 ms) so the behavior is deterministic under the
mock device/transport, not wall-clock dependent.

## Options (`ProtocolSessionOptions`)

| Option | Default | Meaning |
|---|---|---|
| `keepalive_idle_poll_interval` | `0` (off) | Send a `keepalive` control frame every N idle polls. |
| `dpd_idle_poll_interval` | `0` (off) | Send a `dpd_request` probe every N idle polls when none is outstanding. |
| `dead_peer_poll_budget` | `0` (off) | Idle polls to wait for any inbound frame after a probe before declaring the peer dead. |

Responding to an **inbound** `dpd_request` is always on and not gated by these
options. `native_engine.cpp` leaves all three at their disabled defaults pending
live-gateway timing validation (the always-on DPD response keeps the gateway
seeing us as alive).

## Inbound thread additions

- Every successfully decoded frame increments an atomic `inbound_activity`
  counter — the liveness evidence the outbound thread reads.
- On `dpd_request`, the thread immediately replies with
  `send_control(dpd_response)` and emits `dpd.responded`; a write failure is a
  fatal reason.

## Outbound thread additions (`service_liveness`)

Runs on each idle poll (inside the retryable-read branch, before the 1 ms
sleep), using `retryable_read_count` as the idle-poll counter:

1. **Dead-peer detection** — if a probe is outstanding: clear it when
   `inbound_activity` has advanced past the baseline captured at send time;
   otherwise count down `dead_peer_poll_budget`. Exhausting the budget returns
   `transport_closed` ("dead peer detected"), which drives reconnect.
2. **Keepalive** — every `keepalive_idle_poll_interval` idle polls, send a
   `keepalive` control frame, emit `dpd.keepalive`.
3. **DPD probe** — every `dpd_idle_poll_interval` idle polls, if no probe is
   outstanding, send `dpd_request`, mark outstanding, snapshot the activity
   baseline, emit `dpd.request`.

A successful outbound packet read resets `retryable_read_count` but does **not**
clear an outstanding probe — only inbound activity clears it.

## Reconnect

`reconnect()` (unchanged shape from P2) transitions the phase to
`reconnecting`, closes the device, calls `transport_->disconnect()` +
`reset_for_reconnect()` (the production transport closes the TLS stream and
clears cookies/passwords so re-auth is clean), re-authenticates, reconnects
CSTP, reopens the device, and emits `reconnect_started` / `reconnect_succeeded`.
On exceeding `max_reconnects`, `run_packet_loop` closes the device, marks the
session `failed`, and returns the stable `transport_closed` code.

## Verification

`native_protocol_session_test` adds four mock-driven cases:

- `test_inbound_dpd_request_is_answered` — injected `dpd_request` →
  `dpd_response` recorded; loop ends gracefully.
- `test_idle_keepalive_is_emitted` — `keepalive_idle_poll_interval = 2` emits
  keepalives on idle polls until cancellation.
- `test_dead_peer_triggers_reconnect` — silent peer + DPD probe + budget → one
  reconnect, re-auth, and graceful recovery.
- `test_reconnect_exhaustion_fails_with_stable_code` — perpetually dead peer
  exhausts `max_reconnects` and fails with `transport_closed`, phase `failed`.

Command: `ctest --preset windows-release -R "native_protocol_session_test|native_production_transport_test" --output-on-failure`
→ **2/2 pass**; full suite **25/25 pass**.

## ENV-BLOCKED (P3)

- **P3-live-DPD-timing** — calibrating real wall-clock keepalive/DPD intervals
  and dead-peer thresholds against a live ECNU AnyConnect gateway. The mock
  poll-count model proves the state machine; production timer values and the
  enable switch in `native_engine.cpp` are gated on live access.

