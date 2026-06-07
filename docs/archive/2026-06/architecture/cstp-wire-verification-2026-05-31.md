# CSTP Wire Framing Verification — 2026-05-31 (Task P1)

Plan: `docs/superpowers/plans/2026-05-31-native-openconnect-extraction-completion-plan.md`

## Scope

De-risk the data-channel framing by replacing the previous fake test-harness
framing (`[type:1][len_be32:4][payload]`) with the AnyConnect-compatible CSTP
**"STF" record header**, and prove encode/decode correctness with byte-exact
vectors covering every frame type plus partial-read and error boundaries.

## Wire format implemented (public CSTP/AnyConnect description)

Each data-channel record is prefixed with an 8-byte header:

```
offset 0..2 : 'S' 'T' 'F'  (0x53 0x54 0x46)   record magic
offset 3    : 0x01                              version
offset 4..5 : payload length, big-endian uint16 (max 65535)
offset 6    : packet type
offset 7    : 0x00                              reserved
offset 8..  : payload bytes
```

Packet type tags:

| Type | Tag |
|---|---|
| DATA | 0x00 |
| DPD request | 0x03 |
| DPD response | 0x04 |
| DISCONNECT | 0x05 |
| KEEPALIVE | 0x07 |

These constants are facts of the public CSTP/AnyConnect protocol required for
interoperability with the gateway. They were implemented independently in
`src/vpn_engine/protocol/cstp.cpp`; no third-party implementation source,
comments, parser structure, or state machine was copied (clean-room rule).

## Behavior changes

- `encode_cstp_frame` emits the 8-byte STF header; payloads above 65535 bytes
  return `cstp_frame_oversized`.
- `decode_cstp_frame` validates the magic and version. A magic/version mismatch
  returns a hard `cstp_bad_magic` (so the read loop stops rather than spinning);
  a short header or short payload returns `cstp_frame_incomplete` without
  consuming bytes (so the transport can read more and retry).
- `ByteReader` gained `read_be_u16` for the uint16 length field.

## Test evidence

Target: `native_cstp_frame_test` (`tests/native_cstp_frame_test.cpp`).

Vectors added/updated:

- Byte-exact encode for all five frame types (empty payload) + round-trip decode.
- Data frame decode with payload.
- Concatenated keepalive + data frames via `ByteReader`.
- Trailing-bytes-after-frame acceptance.
- Partial payload → `cstp_frame_incomplete`, reader not advanced.
- Partial header (<8 bytes) → `cstp_frame_incomplete`, reader not advanced.
- Bad magic → `cstp_bad_magic`, reader not advanced.
- Oversized payload at encode → `cstp_frame_oversized`.

Also fixed a pre-existing malformed fixture
`tests/fixtures/native_anyconnect/cstp_connect_success.http` that lacked the
HTTP header terminator (blank line), which had been causing the header-parse
sub-block of `native_cstp_frame_test` to fail independently of framing.

Commands and results (Windows, `windows-release`):

```
cmake --build --preset windows-release --target native_cstp_frame_test native_fake_anyconnect_server_test native_production_transport_test
ctest --preset windows-release -R "native_cstp_frame_test|native_fake_anyconnect_server_test|native_production_transport_test|native_protocol_session_test|native_tls_stream_contract_test|native_auth_parser_test|native_url_test|native_engine_contract_test" --output-on-failure
=> 100% tests passed, 0 tests failed out of 8
```

## Verdict

- `framing-verified` against the public CSTP/AnyConnect protocol description and
  internal round-trip/error vectors.
- **Outstanding (environment-blocked):** verification against a live, redacted
  ECNU capture requires ECNU network reachability and credentials, which are not
  available on the current Windows build host. When an ECNU-reachable host is
  available, capture redacted DATA/DPD/KEEPALIVE/DISCONNECT records and add them
  as fixtures to close the live-capture portion. No code change is expected
  unless ECNU uses a non-standard tag/version, in which case the divergence is
  handed back to P2/P3.
