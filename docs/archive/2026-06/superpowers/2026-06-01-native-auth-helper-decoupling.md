# Native Auth Helper Decoupling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make native AnyConnect authentication correct, diagnosable, and independent from privileged helper startup so bad credentials or protocol mismatches fail fast without creating interfaces, writing routes, or starting reconnect floods.

**Architecture:** Split the native flow into a non-privileged user-mode authentication phase and a privileged tunnel/bootstrap phase. The core process authenticates with the gateway and returns a short-lived authenticated session descriptor; the helper/supervisor consumes that descriptor only after auth succeeds and limits itself to CSTP establishment, packet device creation, IP configuration, route/DNS writes, and privileged cleanup.

**Tech Stack:** C++17 native engine, Schannel/Secure Transport TLS streams, existing helper RPC JSON, Electron/Vue desktop RPC, CMake/Ninja tests, current clean-room OpenConnect reference under `reference/openconnect-upstream`.

## 2026-06-01 Multi-Agent Execution Update

This plan is now an executable collaboration plan. Preserve the original task bodies below as implementation detail, but treat this section as the current coordination state.

### Systematic Debugging Root Cause Conclusion

The reconnect flood and wrong-error-path confusion have been narrowed by the completed slices:

- fatal classifier/supervisor stop: native fatal errors such as `auth_protocol_mismatch` now stop the supervisor instead of retrying into a flood.
- protocol diagnostics: auth failures now include safe HTTP response metadata such as status, content type, content length, transfer encoding, decoded body bytes, and bounded body prefix.
- error propagation: protocol errors now preserve `auth_protocol_mismatch` end to end instead of being collapsed into `auth_failed`.
- request shape: XML aggregate-auth POSTs no longer present as form posts.
- chunked response handling: chunked XML responses are decoded instead of being misread as empty 200 responses.

The remaining structural problem is privilege placement: native authentication still happens serially inside the helper-derived privileged worker/supervisor. The current desktop/core path does not authenticate first in user mode and then hand a short-lived session to helper.

The key blocking seam is `ProductionProtocolTransport::connect_cstp()`: it currently assumes it is called on the same transport/TLS/session state that just completed `authenticate()`. That implicit dependency on post-auth transport/TLS state prevents directly consuming a user-mode `NativeAuthSession` in the privileged bootstrap path. Task 6A exists to split out an authenticated CSTP bootstrap primitive without moving helper RPC yet.

### New Overall Goal

Converge helper into a privileged resource tool. Authentication and AnyConnect XML/TLS interaction should live in non-privileged core/desktop wherever possible. The short-term transition may keep the helper/supervisor as owner of the packet loop, CSTP CONNECT, interface creation, routes, and DNS application, but it must be able to start from a pre-authenticated session rather than from plaintext credentials.

### Completed Tasks and Acceptance

| Task | Status | Main files | Verification |
| --- | --- | --- | --- |
| Task 1 - safe diagnostics | Done | `src/vpn_engine/protocol/production_transport.cpp`, `tests/native_production_transport_test.cpp` | `cmake --build build-windows\cpp --target native_production_transport_test`; `.\build-windows\cpp\native_production_transport_test.exe`; passing. Acceptance: live failures can distinguish content length, transfer encoding, decoded body bytes, and bounded body prefix without logging secrets. |
| Task 2 - XML POST request shape | Done | `src/vpn_engine/protocol/production_transport.cpp`, `tests/native_production_transport_test.cpp`, `docs/architecture/native-anyconnect-protocol-requirements.md` | `cmake --build build-windows\cpp --target native_production_transport_test native_auth_parser_test`; `.\build-windows\cpp\native_production_transport_test.exe`; `.\build-windows\cpp\native_auth_parser_test.exe`; passing. Acceptance: init and auth-reply requests use `application/xml; charset=utf-8` and not form URL encoding. |
| Task 3 - chunked HTTP response | Done | `src/vpn_engine/protocol/http.hpp`, `src/vpn_engine/protocol/http.cpp`, `src/vpn_engine/protocol/production_transport.cpp`, `tests/native_production_transport_test.cpp` | `cmake --build build-windows\cpp --target native_production_transport_test native_url_test native_cstp_frame_test`; `.\build-windows\cpp\native_production_transport_test.exe`; `.\build-windows\cpp\native_url_test.exe`; `.\build-windows\cpp\native_cstp_frame_test.exe`; passing. Acceptance: chunked config-auth is decoded, incomplete chunks read more data, malformed chunks surface protocol errors. |
| Task 4 - protocol errors end to end | Done | `src/feedback/feedback.hpp`, `src/feedback/feedback.cpp`, `src/vpn_engine/native_error_contract.hpp`, `src/helper.cpp`, `src/app_api.cpp`, `webui/desktop/shared/desktop-contract.ts`, `webui/src/types/ecnu-vpn.d.ts`, `webui/src/stores/vpn.ts`, `tests/feedback_test.cpp`, `tests/native_helper_session_test.cpp` | `cmake --build build-windows\cpp --target feedback_test native_helper_session_test`; `.\build-windows\cpp\feedback_test.exe`; `.\build-windows\cpp\native_helper_session_test.exe`; passing. Acceptance: `auth_protocol_mismatch` reaches desktop as a protocol error, not a password retry. |
| Task 5 - `NativeAuthenticator` minimal extraction | Done | `src/vpn_engine/protocol/native_authenticator.hpp`, `src/vpn_engine/protocol/native_authenticator.cpp`, `src/vpn_engine/protocol/production_transport.hpp`, `src/vpn_engine/protocol/production_transport.cpp`, `src/vpn_engine/protocol/session.hpp`, `src/vpn_engine/protocol/session.cpp`, `CMakeLists.txt`, `tests/native_authenticator_test.cpp` | `cmake --build build-windows\cpp --target native_authenticator_test native_production_transport_test`; `.\build-windows\cpp\native_authenticator_test.exe`; `.\build-windows\cpp\native_production_transport_test.exe`; passing. Acceptance: user-mode auth can be tested without helper/packet devices and returns `NativeAuthSession`. |
| Task 6A - authenticated CSTP bootstrap primitive | Done | `src/vpn_engine/protocol/session.hpp`, `src/vpn_engine/protocol/session.cpp`, `src/vpn_engine/native_engine.hpp`, `src/vpn_engine/native_engine.cpp`, `tests/native_protocol_session_test.cpp`, `tests/native_engine_contract_test.cpp` | `cmake --build build-windows\cpp --target native_engine_contract_test native_protocol_session_test`; `.\build-windows\cpp\native_engine_contract_test.exe`; `.\build-windows\cpp\native_protocol_session_test.exe`; passing. Acceptance: authenticated bootstrap uses a supplied `NativeAuthSession`/cookie without invoking auth; empty cookies are rejected without corrupting existing session state; active tunnel adoption is rejected instead of replacing a live tunnel. Specification and quality reviews both passed. |
| Task 6B - `NativeAuthSession` JSON codec | Done | `src/vpn_engine/protocol/native_auth_session_json.hpp`, `src/vpn_engine/protocol/native_auth_session_json.cpp`, `tests/native_auth_session_json_test.cpp`, `CMakeLists.txt` | `cmake --build build-windows\cpp --target native_auth_session_json_test`; `.\build-windows\cpp\native_auth_session_json_test.exe`; passing. Acceptance: JSON round-trip preserves required descriptor fields; `schema_version` accepts only exact integer `1`; diagnostics use an allowlist and reject/filter secret-like values; summaries and errors do not leak cookies, tokens, passwords, CSRF/SAML, or other secrets. Specification and security reviews both passed. |
| Task 6B2 - JSON parser header-bound field hardening | Done | JSON parser/header-bound field validation and focused tests | Dual-pass accepted. Acceptance: header-bound fields are hardened against malformed/control-bearing input before session payloads reach helper/supervisor boundaries. |
| Task 6C1 - engine `start_authenticated` | Done | `src/vpn_engine/native_engine.*`, `src/vpn_engine/protocol/session.*`, engine/session tests | Dual-pass accepted. Acceptance: `NativeVpnEngineSession::start_authenticated` starts from an authenticated session; auth-session mode disables legacy reconnect; cookie and user-agent control characters are rejected; logs/status do not leak cookies. |
| Task 6C2a - common supervisor payload codec | Done | common native supervisor payload codec and tests | Dual-pass accepted. Acceptance: payload codec supports password and `auth_session` modes; summaries are redacted; `config.password` is removed from auth-session payloads. |
| Task 6C2b - platform supervisor payload wiring | Done | platform supervisor/native payload wiring and tests | Dual-pass accepted. Acceptance: Windows safe parse/decode fixed message is used; platform payload overload is wired; native `auth_session` dispatch is supported; non-native auth-session requests fail closed. |
| Task 6C3a - Windows helper request-file ACL hardening | Done | helper request-file and Windows security code, helper/security tests | Dual-pass accepted. Acceptance: cookie-bearing auth-session request files are owner-restricted and logs/status redact session payloads. |
| Task 6C3b - helper `native_start_mode=auth_session` | Done | `src/helper.*`, helper internal API, helper/supervisor tests | Dual-pass accepted. Acceptance: helper accepts `native_start_mode=auth_session`, worker requests omit password/config.password, helper internal API moved to `helper_internal.hpp`, and supervisor crash messages are sanitized. |
| Task 6D1 - app_api native auth-first orchestration seam | Done | `src/app_api.cpp`, app API policy tests | Dual-pass accepted. Acceptance: app_api has an auth-first orchestration seam that can return native auth failures before helper startup and can pass serialized authenticated sessions to helper startup. |
| Task 6D2 - real app_api native auth-first wiring | Done | `src/app_api.cpp`, app API policy tests | Dual-pass accepted. Acceptance: service path checks helper first then auth; oneshot fallback authenticates before helper/UAC; helper receives auth-session request; legacy behavior is unchanged. |
| Task 6D3 - desktop connectElevated normal RPC fallback | Done | desktop connect/elevation RPC path and tests | Dual-pass accepted. Acceptance: desktop `connectElevated` now uses normal RPC with `allow_direct_fallback`; disconnect/install/update elevation behavior is unchanged. |

### Current Active Slice

Tasks 6A, 6B, 6B2, 6C1, 6C2a, 6C2b, 6C3a, 6C3b, 6D1, 6D2, and 6D3 are complete and accepted.

Current native desktop behavior:

- Actual native desktop connect now performs user-mode authentication before helper start on the fallback path.
- The service path checks helper availability before auth, then performs auth only after the helper path is known to be usable.
- Native auth failures return before helper start.
- Successful native auth sends a helper `native_start_mode=auth_session` request.
- The helper accepts `auth_session` mode and starts the authenticated tunnel from the supplied session.
- Desktop `connectElevated` now uses normal RPC with `allow_direct_fallback`; disconnect/install/update elevation behavior is unchanged.
- Legacy non-native behavior is unchanged.

Remaining boundary work is follow-up only: service-only helper prepare/cancel parallelism is not implemented yet, adopted-auth reconnect remains disabled until a future reauth design, and live ECNU gateway validation is still required.

### Three-Level Task Tree

**L0 Goal:** move native AnyConnect authentication out of privileged helper startup while preserving current packet-loop functionality and failure diagnostics.

**L1 Work Packages:** protocol authenticated bootstrap; `NativeAuthSession` serialization; helper `start_authenticated`; app API auth-first orchestration; later service-only prepare/cancel behavior; reconnect/reauth semantics; WebUI feedback; security/secret redaction; final integration and validation.

| L1 package | L2 concrete task | Boundary and file scope | Dependencies | Parallelism | Acceptance | Tests |
| --- | --- | --- | --- | --- | --- | --- |
| Protocol authenticated bootstrap | Task 6A: `ProtocolSession` adopts `NativeAuthSession` and connects CSTP from it | `src/vpn_engine/protocol/session.*`, `src/vpn_engine/native_engine.*`, protocol/engine tests only | Task 5 | Complete; coordinate future session edits with 6C1 | No auth call on authenticated path; empty cookie rejected without state damage; active tunnel adopt rejected; legacy start remains | `native_engine_contract_test`, `native_protocol_session_test` |
| `NativeAuthSession` serialization | Task 6B: JSON codec and redaction helpers | `src/vpn_engine/protocol/native_auth_session_json.*`, `CMakeLists.txt`, `tests/native_auth_session_json_test.cpp` | Task 5 type stable | Complete; payload schema is available to 6C3/security | Round-trip required fields; exact `schema_version` validation; diagnostics allowlist and secret filter; never serialize password | `native_auth_session_json_test` |
| JSON parser hardening | Task 6B2: header-bound field hardening | JSON parser/header-bound field validation and focused tests | Task 6B | Complete; dual-pass accepted | Header-bound fields reject malformed/control-bearing input before helper/supervisor payload use | JSON parser/header-bound field tests |
| Engine authenticated start | Task 6C1: engine-only `NativeVpnEngineSession::start_authenticated` | `src/vpn_engine/native_engine.*`, `src/vpn_engine/protocol/session.*`, engine/session tests only | Tasks 6A, 6B | Complete; dual-pass accepted | Engine starts CSTP and packet loop from a supplied session without auth; auth-session mode disables legacy reconnect; cookie/user-agent control characters are rejected; cookie is not leaked | `native_engine_contract_test`, `native_protocol_session_test` |
| Common supervisor payload codec | Task 6C2a: password/auth-session payload codec | common supervisor payload codec and tests | Tasks 6B, 6C1 | Complete; dual-pass accepted | Password and `auth_session` modes encode/decode safely; redacted summary is available; `config.password` is removed in auth-session mode | supervisor payload codec tests |
| Platform supervisor payload wiring | Task 6C2b: platform payload dispatch | platform supervisor/native payload wiring and tests | Task 6C2a | Complete; dual-pass accepted | Windows safe parse/decode fixed message is used; platform payload overload exists; native auth-session dispatch works; non-native auth-session fails closed | `native_helper_session_test`, platform payload tests |
| Windows request-file security | Task 6C3a: ACL/security hardening for auth-session request files | helper request-file and Windows security code, helper/security tests | Task 6C2b | Complete; dual-pass accepted | Auth-session request files are owner-restricted; logs/status redact cookie/session payloads | `native_helper_session_test`, Windows request-file/security tests |
| helper `start_authenticated` | Task 6C3b: helper `native_start_mode=auth_session` | `src/helper.*`, `helper_internal.hpp`, helper tests | Task 6C3a | Complete; dual-pass accepted | Helper accepts `native_start_mode=auth_session`; worker request has no password/config.password; helper internal API moved to `helper_internal.hpp`; supervisor crash message is sanitized | `native_helper_session_test` |
| app API auth-first seam | Task 6D1: native auth-first orchestration seam | `src/app_api.cpp`, app API tests | Tasks 6C3a, 6C3b | Complete; dual-pass accepted | Native auth failure can return before helper start; success can send auth-session request without password | `app_api_runtime_policy_test` |
| app API auth-first wiring | Task 6D2: real native auth-first app_api wiring | `src/app_api.cpp`, app API tests | Task 6D1 | Complete; dual-pass accepted | Service path checks helper first then auth; oneshot fallback auth runs before helper/UAC; helper receives auth-session request; legacy path unchanged | `app_api_runtime_policy_test`, `backend_resolver_test` |
| desktop RPC fallback | Task 6D3: `connectElevated` uses normal RPC fallback | desktop connect/elevation RPC path and tests | Task 6D2 | Complete; dual-pass accepted | `connectElevated` uses normal RPC with `allow_direct_fallback`; disconnect/install/update elevation unchanged | desktop RPC/elevation tests |
| service-only helper prepare | Task 6C3d: non-mutating service prepare/cancel | `src/app_api.cpp`, `src/platform/common/backend_resolver.*`, `src/platform/common/helper_client.*`, resolver tests | Auth-first path stable | Future optimization; not implemented | Service-only prepare does not send helper `start` or mutate network state; cancellation is non-mutating | `backend_resolver_test`, app API policy tests |
| cancel/prepare behavior | Task 8C: cancellation and one-shot UAC policy | `src/app_api.cpp`, backend resolver/client tests | Task 6C3d | Later optimization; parallel only after prepare contract exists | Installed helper prepare may overlap auth; one-shot elevation waits until auth succeeds | `backend_resolver_test`, app API policy tests |
| reconnect/reauth semantics | Task 9A: classify cookie retry vs reauth-required | `src/vpn.cpp`, `src/vpn_engine/native_error_contract.hpp`, session state/protocol tests | Task 6C3b | Parallel with WebUI and security after code names freeze | Transient CSTP failures retry; 401/403/auth-shaped CSTP rejection stops as `reauth_required` | `native_protocol_session_test`, `native_helper_session_test` |
| WebUI feedback | Task 10A: type contract and store mapping | `webui/desktop/shared/desktop-contract.ts`, `webui/src/types/ecnu-vpn.d.ts`, `webui/src/stores/vpn.ts` | Task 4 code names | Parallel with Tasks 6-9 | Protocol mismatch does not open password retry; reauth-required does | `cd webui; npm run typecheck` |
| WebUI feedback | Task 10B: progress states | `webui/src/pages/DashboardPage.vue`, `webui/src/components/ErrorDialog.vue`, store/types | Task 6D stage names | Parallel after stage names freeze | UI shows auth, helper prepare, helper start, CSTP, and network config phases | `cd webui; npm run typecheck` |
| security/secret redaction | Task 12A: audit payloads and logs | helper/app/session JSON files, `docs/security/native-openconnect-replacement-review-2026-05-31.md`, helper tests | Task 6B and 6C3a/6C3b | Parallel with reconnect after payload schema exists | No password in native helper request files/logs; cookie redacted in logs and persisted status | `native_helper_session_test`, manual diff/log review |
| final integration | Task 11A: one-shot live validation script | `scripts/validate-native-auth-once.ps1`, validation docs | Task 1-4; stronger after Task 8 | Parallel with docs; live run is final gate | One auth attempt, no flood, redacted diagnostic output | script plus manual ECNU validation |
| final integration | Task 12B: remove desktop plaintext native helper path | `src/app_api.cpp`, `src/helper.cpp`, `src/vpn.cpp`, tests and security docs | Tasks 8 and 9 | Sequential final cleanup | Desktop native start requires auth session; CLI fallback policy documented | `native_helper_session_test`, app API policy tests, merge-prep validation |

---

## Current Diagnosis

The reconnect flood is fixed in shape: the pasted log from `2026-06-01 00:13:55` and `00:14:17` shows one native attempt, `auth_protocol_mismatch`, then `Native VPN failure is non-retryable ... supervisor will not reconnect`. That part is behaving as intended.

The remaining problem is still mostly in the extracted/refactored/custom OpenConnect logic, but not only there.

### Evidence

- `src/vpn_engine/protocol/production_transport.cpp:310-328` sends XML `<config-auth ...>` bodies with `Content-Type: application/x-www-form-urlencoded`. That is internally inconsistent and does not match `reference/openconnect-upstream/auth.c`, where XML post bodies use `application/xml; charset=utf-8`.
- `docs/architecture/native-anyconnect-protocol-requirements.md:78-85` says the init POST must be XML and must not be treated like the old HTML/form login flow.
- The pasted log shows `http_status=200 content_type=text/xml; charset=utf-8 body_bytes=0`. This narrows the live failure to the init response body path, but it does not prove the server returned a true empty body.
- `src/vpn_engine/protocol/production_transport.cpp:784-876` only supports `Content-Length` bodies or "whatever bytes are already buffered after headers". It does not decode `Transfer-Encoding: chunked`. If ECNU returns chunked XML and the first TLS read contains only headers, the current code reports `body_bytes=0` and clears the buffer.
- `src/vpn_engine/native_error_contract.hpp:57-74` and `src/feedback/feedback.cpp:108-110` collapse any code/message containing `auth` to `auth_failed`. That can convert `auth_protocol_mismatch` into a password error in helper/desktop paths.
- `src/helper.cpp:769-773` checks `status == vpn::kVpnInitialConnectFailedExitCode` before returning the captured native failure code. If native startup returns the legacy initial-connect failure exit code, helper returns `auth_failed` even when `native-session-state.json` contains `auth_protocol_mismatch`.
- `src/vpn_engine/native_engine.cpp:291-311` authenticates before CSTP and packet-device creation inside the supervisor-owned session. Authentication itself does not need administrator/root privileges, but the current desktop flow reaches it only after helper/service/oneshot startup.

### Working Hypotheses To Test

1. **H1 - request shape mismatch:** ECNU ignores or rejects the XML init because we label it as `application/x-www-form-urlencoded`. Expected fix: XML POSTs use `application/xml; charset=utf-8`, and ECNU returns a non-empty `<config-auth>`.
2. **H2 - response reader bug:** ECNU is already returning XML with `Transfer-Encoding: chunked`, but `read_http_response()` reports an empty body because it has no chunked decoder. Expected fix: diagnostics show `transfer_encoding=chunked`, and a chunked unit test fails before adding a decoder.
3. **H3 - error propagation bug:** even when the native engine has the right internal code, helper/app/frontend normalize it back to `auth_failed`. Expected fix: `auth_protocol_mismatch` reaches the renderer as its own error type with a protocol/debugging message, not the password retry path.

The implementation order below tests these hypotheses before the larger architecture split.

---

## Target Architecture

### Components

- `NativeAuthenticator`: user-mode component that owns TLS auth requests, XML aggregate-auth state, cookies, challenge/group events, and sanitized diagnostics. It does not create packet devices, write routes, or require helper privileges.
- `NativeAuthSession`: serializable, secret-minimized descriptor passed from core to helper after authentication. It contains server identity, user-agent, auth cookie/session token, selected group, negotiated auth metadata, creation time, and redacted diagnostics. It must not contain the plaintext password.
- `NativeTunnelBootstrap`: helper/supervisor entry point that consumes `NativeAuthSession`, performs CSTP CONNECT, parses tunnel metadata, creates the packet device, applies IP/routes/DNS, and starts the packet loop.
- `DesktopConnectOrchestrator`: app API flow that runs authentication and helper readiness in parallel where safe. It returns auth failures immediately and cancels or releases helper preparation.
- `NativeSupervisor`: durable privileged owner of the packet loop. It can retry transient CSTP/transport failures using the same auth session. It must stop and surface `reauth_required` if the cookie/session expires.

### Concurrency Policy

- Current stage: do not run full helper `prepare`/`cancel` in parallel with auth. The staged product path is auth-first, then helper start with `native_start_mode=auth_session`.
- Later stage only: when service-only non-mutating prepare is explicitly implemented and tested, installed helper/service availability may be prepared in parallel. The helper must not create interfaces or write routes until it receives `NativeAuthSession`.
- One-shot elevated helper unavailable: default to auth-first, then trigger UAC/oneshot only after auth succeeds. This avoids asking for elevation when the password/protocol is already wrong.
- Existing service installation flow: may prepare service startup in parallel only when it does not display UAC or mutate network state.
- Authentication failure: cancel helper preparation, return the auth/protocol error, do not spawn a native supervisor, do not write native session state as running, and do not retry.
- Transient transport failure after network-ready: supervisor may retry CSTP with the current session token according to `retry_limit`.
- Session-token rejection during reconnect: stop supervisor and emit `reauth_required`; do not prompt from helper because helper must not own credentials.

---

## Phase Overview

1. **Phase 0 - Safety and observability:** add tests and logs that separate request-shape errors, chunked/empty-body errors, and error-code collapse.
2. **Phase 1 - Immediate native auth correctness:** fix XML POST headers, response-body reading, and end-to-end protocol error propagation.
3. **Phase 2 - Extract user-mode authenticator:** move auth out of `ProductionProtocolTransport::authenticate()` into a standalone component with focused tests.
4. **Phase 3 - Define helper bootstrap contract:** stop passing plaintext password to native helper start; pass an authenticated session descriptor after auth succeeds.
5. **Phase 4 - Desktop orchestration:** run user-mode auth and safe helper preparation concurrently, with cancellation and fast auth failure return.
6. **Phase 5 - Reconnect lifecycle:** define what the supervisor can retry with a cookie and when it must request reauthentication.
7. **Phase 6 - Validation and release:** test Windows/macOS paths, fake gateway scenarios, real ECNU single-attempt validation, and staged binary packaging.

---

### Task 1: Add Wire Diagnostics Before More Fixes

**Files:**
- Modify: `src/vpn_engine/protocol/production_transport.cpp`
- Modify: `tests/native_production_transport_test.cpp`
- Test: `build-windows/cpp/native_production_transport_test.exe`

**Meaning:** Make the next live ECNU run distinguish a real empty XML response from a chunked-body reader bug or a request header mismatch.

- [ ] **Step 1: Add a response summary test for header fields**

Add a test that drives an init response with `Content-Length: 0`, `Content-Type: text/xml`, and `Transfer-Encoding: chunked` separately. The protocol mismatch message must include `http_status`, `content_type`, `content_length`, `transfer_encoding`, `body_bytes`, and `body_prefix` when non-empty.

Expected test shape:

```cpp
bool protocol_mismatch_summary_includes_body_headers() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  MockTlsStream stream;
  stream.push_read_text(
      "HTTP/1.1 200 OK\r\n"
      "Content-Type: text/xml; charset=utf-8\r\n"
      "Content-Length: 0\r\n"
      "\r\n");

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  bool ok = true;
  ok = expect(!auth.ok, "empty XML response must fail auth") && ok;
  ok = expect(auth.error_code == "auth_protocol_mismatch",
              "empty XML response should stay protocol mismatch") && ok;
  ok = expect(auth.error_message.find("http_status=200") != std::string::npos,
              "summary should include status") && ok;
  ok = expect(auth.error_message.find("content_length=0") != std::string::npos,
              "summary should include content length") && ok;
  ok = expect(auth.error_message.find("body_bytes=0") != std::string::npos,
              "summary should include body size") && ok;
  return ok;
}
```

- [ ] **Step 2: Extend `summarize_http_response()`**

Add safe header extraction:

```cpp
if (const std::string *cl = response.header_ci("content-length"))
  out << " content_length=" << *cl;
if (const std::string *te = response.header_ci("transfer-encoding"))
  out << " transfer_encoding=" << *te;
```

Sanitize both values by stripping control characters before adding them to logs.

- [ ] **Step 3: Add outgoing request summary logging**

Before writing the XML init request, log method/path/content type/body length and the first XML tag name. Do not log username, password, cookie, or session token.

Example log fields:

```text
[native-engine] auth.request: method=POST path=/ content_type=application/xml body_bytes=...
```

- [ ] **Step 4: Run test**

Run:

```powershell
cmake --build build-windows\cpp --target native_production_transport_test
.\build-windows\cpp\native_production_transport_test.exe
```

Expected: the new test fails before diagnostics are added and passes after diagnostics include the extra headers.

**Acceptance Criteria:**
- A live failure can show whether ECNU sent `Content-Length: 0`, `Transfer-Encoding: chunked`, or a non-empty non-config body.
- No password/cookie/session token appears in any new log line.

---

### Task 2: Fix XML POST Request Shape

**Files:**
- Modify: `src/vpn_engine/protocol/production_transport.cpp`
- Modify: `tests/native_production_transport_test.cpp`
- Review: `reference/openconnect-upstream/auth.c`
- Review: `docs/architecture/native-anyconnect-protocol-requirements.md`
- Test: `build-windows/cpp/native_production_transport_test.exe`

**Meaning:** Bring the init/auth-reply request framing back in line with AnyConnect XML aggregate-auth and the local OpenConnect reference.

- [ ] **Step 1: Add failing tests for XML content type**

In `authenticate_success_runs_aggregate_auth_and_returns_cookie()`, assert both init and auth-reply requests carry XML content type and do not carry form content type:

```cpp
ok = expect(contains(init_request,
                     "Content-Type: application/xml; charset=utf-8\r\n"),
            "init XML POST should use application/xml content type") && ok;
ok = expect(init_request.find("application/x-www-form-urlencoded") ==
                std::string::npos,
            "init XML POST must not be labeled as form-url-encoded") && ok;
ok = expect(contains(reply_request,
                     "Content-Type: application/xml; charset=utf-8\r\n"),
            "auth-reply XML POST should use application/xml content type") && ok;
```

- [ ] **Step 2: Fix request builder**

Change `make_xml_post_request()`:

```cpp
out << "Content-Type: application/xml; charset=utf-8\r\n";
```

Keep `Accept-Encoding: identity`, `X-Transcend-Version: 1`, `X-Aggregate-Auth: 1`, and `Connection: keep-alive`.

- [ ] **Step 3: Align docs if needed**

`docs/architecture/native-anyconnect-protocol-requirements.md` currently says `text/xml`. Update it to say `application/xml; charset=utf-8` as the implementation target, with a note that the parser accepts `text/xml` responses from the gateway.

- [ ] **Step 4: Run tests**

Run:

```powershell
cmake --build build-windows\cpp --target native_production_transport_test native_auth_parser_test
.\build-windows\cpp\native_production_transport_test.exe
.\build-windows\cpp\native_auth_parser_test.exe
```

Expected: both pass.

**Acceptance Criteria:**
- Captured outgoing init and auth-reply requests are XML posts by method, body, and `Content-Type`.
- Current fake aggregate-auth success path still passes.

---

### Task 3: Add Chunked HTTP Response Support

**Files:**
- Modify: `src/vpn_engine/protocol/http.hpp`
- Modify: `src/vpn_engine/protocol/http.cpp`
- Modify: `src/vpn_engine/protocol/production_transport.cpp`
- Modify: `tests/native_production_transport_test.cpp`
- Test: `build-windows/cpp/native_production_transport_test.exe`

**Meaning:** Avoid misdiagnosing a valid chunked `<config-auth>` response as an empty 200 XML response.

- [ ] **Step 1: Add failing chunked auth test**

Create a response where headers arrive first and the chunked body arrives in later TLS reads:

```cpp
bool chunked_config_auth_response_is_decoded() {
  using ecnuvpn::vpn_engine::protocol::ProductionProtocolTransport;

  const std::string body =
      "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n"
      "<config-auth client=\"vpn\" type=\"auth-request\" "
      "aggregate-auth-version=\"2\">"
      "<auth id=\"main\"><form>"
      "<input type=\"password\" name=\"password\"></input>"
      "</form></auth></config-auth>";

  MockTlsStream stream;
  stream.push_read_text(
      "HTTP/1.1 200 OK\r\n"
      "Content-Type: text/xml; charset=utf-8\r\n"
      "Transfer-Encoding: chunked\r\n"
      "\r\n");
  stream.push_read_text("10\r\n" + body.substr(0, 16) + "\r\n");
  std::ostringstream last;
  last << std::hex << (body.size() - 16) << "\r\n"
       << body.substr(16) << "\r\n0\r\n\r\n";
  stream.push_read_text(last.str());
  stream.push_read_text(login_post_ok("webvpn=SESSION"));

  ProductionProtocolTransport transport(&stream);
  auto auth = transport.authenticate(options());

  bool ok = true;
  ok = expect(auth.ok, "chunked config-auth response should authenticate") && ok;
  ok = expect(auth.cookie.find("webvpn=SESSION") != std::string::npos,
              "chunked auth should preserve session cookie") && ok;
  return ok;
}
```

- [ ] **Step 2: Implement chunk parser**

Add a helper in `http.cpp` or a private helper in `production_transport.cpp`:

```cpp
ValidationResult decode_chunked_body(const std::vector<std::uint8_t> &wire,
                                     std::size_t body_start,
                                     std::string *body,
                                     std::size_t *consumed);
```

Rules:
- Parse hexadecimal chunk sizes.
- Support optional chunk extensions after `;`.
- Require `\r\n` after each chunk data block.
- Stop at zero-size chunk and consume trailing headers until `\r\n\r\n`.
- Return `http_chunk_incomplete` when more bytes are needed.
- Return `http_chunk_invalid` for malformed chunk framing.
- Enforce `kMaxHttpBodyBytes`.

- [ ] **Step 3: Use chunk parser in `read_http_response()`**

In `read_http_response(false, ...)`, if `Transfer-Encoding` contains `chunked`, keep reading until `decode_chunked_body()` returns complete, then build `HttpResponse` from the parsed headers and decoded body.

Pseudo-code:

```cpp
if (is_chunked(header_response)) {
  while (true) {
    std::string decoded;
    std::size_t consumed = 0;
    ValidationResult chunked =
        decode_chunked_body(read_buffer_, body_start, &decoded, &consumed);
    if (chunked.ok) {
      header_response.body = std::move(decoded);
      *response = std::move(header_response);
      read_buffer_.erase(read_buffer_.begin(),
                         read_buffer_.begin() + consumed);
      return {};
    }
    if (chunked.code != "http_chunk_incomplete")
      return chunked;
    ValidationResult read = read_more();
    if (!read.ok)
      return read;
  }
}
```

- [ ] **Step 4: Run tests**

Run:

```powershell
cmake --build build-windows\cpp --target native_production_transport_test native_url_test native_cstp_frame_test
.\build-windows\cpp\native_production_transport_test.exe
.\build-windows\cpp\native_url_test.exe
.\build-windows\cpp\native_cstp_frame_test.exe
```

Expected: chunked test passes; existing stale-buffer CSTP retry tests still pass.

**Acceptance Criteria:**
- A chunked ECNU XML response is parsed into `response.body`.
- Incomplete chunks read more data instead of returning `auth_protocol_mismatch`.
- Malformed chunks produce `http_chunk_invalid`, not `auth_failed`.

---

### Task 4: Preserve Protocol Errors End-To-End

**Files:**
- Modify: `src/feedback/feedback.hpp`
- Modify: `src/feedback/feedback.cpp`
- Modify: `src/vpn_engine/native_error_contract.hpp`
- Modify: `src/helper.cpp`
- Modify: `src/app_api.cpp`
- Modify: `webui/desktop/shared/desktop-contract.ts`
- Modify: `webui/src/types/ecnu-vpn.d.ts`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `tests/feedback_test.cpp`
- Modify: `tests/native_helper_session_test.cpp`
- Test: `build-windows/cpp/feedback_test.exe`
- Test: `build-windows/cpp/native_helper_session_test.exe`

**Meaning:** Stop showing protocol mismatch as "password wrong"; only real credential rejection should use the password retry path.

- [ ] **Step 1: Add canonical code**

In `feedback.hpp`, add:

```cpp
inline constexpr const char *kAuthProtocolMismatch = "auth_protocol_mismatch";
inline constexpr const char *kReauthRequired = "reauth_required";
```

In `feedback.cpp::is_canonical()`, include both.

In `feedback.cpp::info_for()`:

```cpp
if (canonical == code::kAuthProtocolMismatch)
  return {canonical, false,
          "Open logs and report the gateway response details."};
if (canonical == code::kReauthRequired)
  return {canonical, true,
          "Sign in again to renew the VPN session."};
```

- [ ] **Step 2: Make native mapping explicit**

In `native_error_contract.hpp`, return `auth_protocol_mismatch` before the generic `contains(hint, "auth")` fallback:

```cpp
if (code == "auth_protocol_mismatch")
  return "auth_protocol_mismatch";
if (code == "reauth_required")
  return "reauth_required";
```

- [ ] **Step 3: Reorder helper failure handling**

In `helper.cpp:741-780`, return captured native failure before the legacy `kVpnInitialConnectFailedExitCode` branch:

```cpp
if (!native_failure_code.empty()) {
  logger::warn("Native VPN connection failed with code: " +
               native_failure_code);
  return make_error(native_failure_message.empty()
                        ? "The native VPN engine failed to establish the connection."
                        : native_failure_message,
                    native_failure_code);
}
if (status == vpn::kVpnInitialConnectFailedExitCode) {
  logger::warn("Initial VPN connection failed before network was ready");
  return make_error("VPN authentication failed or the server rejected the connection.",
                    "auth_failed");
}
```

- [ ] **Step 4: Stop app API from normalizing protocol mismatch to auth_failed**

Because `app_api.cpp::error()` calls `feedback::lookup_error()`, this is mostly handled by Step 1. Add a unit or integration test if an app API test target exists; otherwise cover through `feedback_test`.

- [ ] **Step 5: Update desktop error contract**

Add `auth_protocol_mismatch` and `reauth_required` to:

```ts
export type VpnErrorType = ... | 'auth_protocol_mismatch' | 'reauth_required'
```

Add entries to `contractErrorMap`:

```ts
auth_protocol_mismatch: {
  error_type: 'auth_protocol_mismatch',
  message: 'VPN 网关返回的认证协议与当前原生实现不匹配。',
  recommended_action: 'view_logs',
  recoverable: false,
},
reauth_required: {
  error_type: 'reauth_required',
  message: 'VPN 登录会话已失效，请重新认证。',
  recommended_action: 'retry_password',
  recoverable: true,
},
```

Do not set `lastFailedConnectMode` for `auth_protocol_mismatch`; keep that behavior only for `auth_failed` and `reauth_required`.

- [ ] **Step 6: Add tests**

`feedback_test.cpp`:

```cpp
expect(feedback::resolve_error_code("auth_protocol_mismatch", "") ==
           "auth_protocol_mismatch",
       "auth_protocol_mismatch must not collapse to auth_failed");
expect(feedback::lookup_error("auth_protocol_mismatch", "").recoverable == false,
       "protocol mismatch should not prompt password retry");
```

`native_helper_session_test.cpp`:

```cpp
ok = expect(store::map_native_error_to_contract_code(
                "auth_protocol_mismatch",
                "gateway did not return config-auth") ==
                "auth_protocol_mismatch",
            "auth protocol mismatch should pass through") && ok;
```

- [ ] **Step 7: Run tests**

Run:

```powershell
cmake --build build-windows\cpp --target feedback_test native_helper_session_test
.\build-windows\cpp\feedback_test.exe
.\build-windows\cpp\native_helper_session_test.exe
```

Expected: both pass, and a forced `auth_protocol_mismatch` response reaches desktop as `auth_protocol_mismatch`.

**Acceptance Criteria:**
- Protocol mismatch is never displayed as wrong password.
- Wrong password still returns `auth_failed` and still opens the password retry flow.

---

### Task 5: Extract `NativeAuthenticator`

**Status:** Done as minimal extraction. Keep the detailed steps below as historical implementation notes and as regression-test guidance for future refactors.

**Files:**
- Create: `src/vpn_engine/protocol/native_authenticator.hpp`
- Create: `src/vpn_engine/protocol/native_authenticator.cpp`
- Modify: `src/vpn_engine/protocol/production_transport.hpp`
- Modify: `src/vpn_engine/protocol/production_transport.cpp`
- Modify: `src/vpn_engine/protocol/session.hpp`
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `CMakeLists.txt`
- Create: `tests/native_authenticator_test.cpp`
- Test: new `native_authenticator_test`

**Meaning:** Authentication becomes a separately testable user-mode unit. It can run before helper startup and can be reused by desktop, CLI, and future reauth orchestration.

- [ ] **Step 1: Define auth data types**

`native_authenticator.hpp`:

```cpp
#pragma once

#include "config.hpp"
#include "vpn_engine/engine.hpp"
#include "vpn_engine/protocol/session.hpp"

#include <chrono>
#include <map>
#include <string>

namespace ecnuvpn::vpn_engine::protocol {

struct NativeAuthSession {
  ParsedVpnUrl server;
  std::string username;
  std::string useragent;
  std::string cookie_header;
  std::string selected_group;
  std::string auth_method;
  std::chrono::system_clock::time_point created_at{};
  std::map<std::string, std::string> diagnostics;
};

struct NativeAuthRequest {
  Config config;
  std::string plaintext_password;
  EventSink *events = nullptr;
};

class NativeAuthenticator {
public:
  explicit NativeAuthenticator(ProtocolTransport *transport);
  ValidationResult authenticate(const NativeAuthRequest &request,
                                NativeAuthSession *session);

private:
  ProtocolTransport *transport_;
};

} // namespace ecnuvpn::vpn_engine::protocol
```

- [ ] **Step 2: Move auth loop out of `ProductionProtocolTransport::authenticate()`**

Keep request construction, XML parsing, cookie jar, and HTTP response parsing in transport initially, but expose one narrow method:

```cpp
ValidationResult ProductionProtocolTransport::authenticate_native(
    const ProtocolSessionOptions &options,
    NativeAuthSession *session);
```

`ProtocolTransport::authenticate()` may remain for compatibility during this task, internally calling the new method and returning `AuthResult`.

- [ ] **Step 3: Add authenticator tests**

`tests/native_authenticator_test.cpp` should cover:
- success returns `NativeAuthSession.cookie_header`;
- wrong password returns `auth_failed`;
- non-config XML/HTML/empty response returns `auth_protocol_mismatch`;
- no plaintext password appears in `session.diagnostics` or error message;
- authenticator does not instantiate `PacketDevice`.

- [ ] **Step 4: Wire CMake**

Add:

```cmake
add_executable(native_authenticator_test
    tests/native_authenticator_test.cpp
    src/vpn_engine/protocol/native_authenticator.cpp
    src/vpn_engine/protocol/production_transport.cpp
    src/vpn_engine/protocol/auth.cpp
    src/vpn_engine/protocol/http.cpp
    src/vpn_engine/protocol/cstp.cpp
    src/vpn_engine/protocol/url.cpp
)
target_include_directories(native_authenticator_test PRIVATE src include)
add_test(NAME native_authenticator_test COMMAND native_authenticator_test)
```

- [ ] **Step 5: Run tests**

Run:

```powershell
cmake --build build-windows\cpp --target native_authenticator_test native_production_transport_test
.\build-windows\cpp\native_authenticator_test.exe
.\build-windows\cpp\native_production_transport_test.exe
```

Expected: new authenticator tests pass and old transport tests still pass.

**Acceptance Criteria:**
- A caller can authenticate without constructing `NativeVpnEngineSession`.
- The auth unit has no dependency on helper/service/platform packet devices.

---

### Task 6: Split Native Session Startup Into Authenticated Bootstrap

**Status:** Completed through the auth-first helper/app/desktop execution path. Task 6A authenticated CSTP bootstrap primitive, Task 6B `NativeAuthSession` JSON codec, Task 6B2 parser hardening, Task 6C1 engine start-authenticated, Task 6C2a/6C2b supervisor payload work, Task 6C3a request-file ACL hardening, Task 6C3b helper auth-session mode, Task 6D1 app_api seam, Task 6D2 real app_api wiring, and Task 6D3 desktop fallback are done and dual-pass accepted. Do not jump directly to full helper prepare/cancel parallelism; that remains future Task 6C3d work.

**Files:**
- Modify: `src/vpn_engine/native_engine.hpp`
- Modify: `src/vpn_engine/native_engine.cpp`
- Modify: `src/vpn_engine/protocol/session.hpp`
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `tests/native_engine_contract_test.cpp`
- Modify: `tests/native_protocol_session_test.cpp`

**Meaning:** The native engine can start from a pre-authenticated cookie/session descriptor, so helper/supervisor no longer need plaintext password for the native path.

- [ ] **Step 1: Add session bootstrap API**

In `ProtocolSession`:

```cpp
ValidationResult adopt_auth_session(const NativeAuthSession &auth_session);
ValidationResult connect_cstp_authenticated(const NativeAuthSession &auth_session,
                                            TunnelMetadata *metadata);
```

Behavior:
- `adopt_auth_session()` sets `authenticated_ = true` and `cookie_ = auth_session.cookie_header`.
- It validates `cookie_header` is non-empty.
- It does not contact the network.

- [ ] **Step 2: Add native engine start overload**

In `NativeVpnEngineSession`:

```cpp
ValidationResult start_authenticated(
    const protocol::NativeAuthSession &auth_session);
```

Implementation uses existing `connect_cstp`, packet-device creation, and packet-loop startup. It must not call `protocol_session->authenticate()`.

- [ ] **Step 3: Keep legacy `start()` path**

Keep `start()` for CLI and tests during migration. Implement it as:

```cpp
ValidationResult auth = protocol_session->authenticate();
if (!auth.ok) return fail(auth);
return start_authenticated(protocol_session->auth_session_snapshot());
```

If this snapshot helper is awkward, keep duplicated glue only temporarily inside Task 6 and remove it in Task 8.

- [ ] **Step 4: Add tests**

`native_engine_contract_test.cpp`:
- mock transport returns failure if `authenticate()` is called;
- pass a `NativeAuthSession` with cookie;
- expect engine calls `connect_cstp(cookie)` and starts packet loop.

`native_protocol_session_test.cpp`:
- `adopt_auth_session()` rejects empty cookie as `auth_cookie_missing`;
- `connect_cstp_authenticated()` does not call transport auth.

- [ ] **Step 5: Run tests**

Run:

```powershell
cmake --build build-windows\cpp --target native_engine_contract_test native_protocol_session_test
.\build-windows\cpp\native_engine_contract_test.exe
.\build-windows\cpp\native_protocol_session_test.exe
```

**Acceptance Criteria:**
- Native engine can establish CSTP and packet loop from a pre-authenticated session.
- Existing `start()` behavior still works until desktop/helper migration is complete.

**Completed 6A Acceptance Notes:**
- Authenticated CSTP bootstrap accepts a supplied `NativeAuthSession` and does not call native auth again.
- Empty cookie adoption is rejected as an auth-cookie error and does not damage existing authenticated state.
- Active tunnel adoption is rejected so a live tunnel cannot be silently replaced.
- Specification and quality validation both passed with `native_engine_contract_test` and `native_protocol_session_test`.

**Completed 6B Acceptance Notes:**
- `NativeAuthSession` JSON codec exists in `src/vpn_engine/protocol/native_auth_session_json.*`.
- `schema_version` validation accepts only exact integer `1`; strings, booleans, floats, missing values, and unsupported integers are rejected deterministically.
- Diagnostics serialization uses an allowlist and filters/rejects secret-like keys and values; summaries and errors do not leak cookies, passwords, tokens, CSRF, SAML, or session material.
- Specification and security validation both passed with `native_auth_session_json_test`.

**Completed 6B2 Acceptance Notes:**
- JSON parser header-bound field hardening is complete and dual-pass accepted.
- Header-bound fields reject malformed/control-bearing input before session descriptors can cross helper or supervisor boundaries.

**Completed 6C1 Acceptance Notes:**
- `NativeVpnEngineSession::start_authenticated` is complete and dual-pass accepted.
- Auth-session mode disables the old reconnect behavior.
- Cookie and user-agent control characters are rejected.
- Cookie/session material is not leaked in logs, status, summaries, or test diagnostics.

**Completed 6C2a Acceptance Notes:**
- The common supervisor payload codec is complete and dual-pass accepted.
- The codec supports password mode and `auth_session` mode.
- Redacted summaries are available for diagnostics.
- `config.password` is removed from auth-session payloads.

**Completed 6C2b Acceptance Notes:**
- Platform supervisor payload wiring is complete and dual-pass accepted.
- Windows uses the safe parse/decode fixed message path.
- Platform payload overloads carry the auth-session payload.
- Native auth-session dispatch is wired.
- Non-native auth-session requests fail closed.

**Task 6C3 Split Strategy:**
1. 6C3a: Windows request-file ACL hardening for cookie-bearing auth-session request files is complete and dual-pass accepted.
2. 6C3b: helper `native_start_mode=auth_session` is complete and dual-pass accepted; worker requests omit password/config.password, helper internal API moved to `helper_internal.hpp`, and supervisor crash messages are sanitized.
3. 6D1: app_api native auth-first orchestration seam is complete and dual-pass accepted.
4. 6D2: real app_api native auth-first wiring is complete and dual-pass accepted; service path checks helper first then auth, oneshot fallback authenticates before helper/UAC, helper receives `auth_session`, and legacy behavior is unchanged.
5. 6D3: desktop `connectElevated` normal RPC with `allow_direct_fallback` is complete and dual-pass accepted; disconnect/install/update elevation behavior is unchanged.
6. 6C3d: service-only non-mutating prepare/cancel remains a later optimization after the auth-first path is stable.

Current product behavior: actual native desktop connect now performs user-mode auth before helper start on fallback; service path checks helper availability before auth; auth failures return before helper start; successful auth sends an `auth_session` request; the helper accepts that request and starts an authenticated tunnel.

Do not implement service helper prepare/cancel parallelism as part of the completed auth-first path. That remains a later optimization after live validation and reauth semantics are settled.

---

### Task 7: Define Native Helper Bootstrap RPC

**Status:** Renumbered by the execution update. The JSON codec portion is complete as Task 6B, JSON parser header-bound field hardening is complete as Task 6B2, engine authenticated start is complete as Task 6C1, supervisor payload work is complete as Tasks 6C2a/6C2b, Windows request-file ACL hardening is complete as Task 6C3a, helper `native_start_mode=auth_session` is complete as Task 6C3b, app_api auth-first seam/wiring is complete as Tasks 6D1/6D2, and desktop `connectElevated` direct fallback is complete as Task 6D3. Service-only non-mutating prepare/cancel remains future Task 6C3d work. Keep the detailed notes below as implementation context, but do not treat the old Task 7 as a single parallelizable unit.

**Files:**
- Modify: `src/helper.cpp`
- Modify: `src/helper.hpp`
- Modify: `src/vpn.cpp`
- Modify: `src/vpn.hpp`
- Modify: `src/vpn_engine/native_session_store.hpp`
- Modify: `src/vpn_engine/native_session_store.cpp`
- Create: `src/vpn_engine/protocol/native_auth_session_json.hpp`
- Create: `src/vpn_engine/protocol/native_auth_session_json.cpp`
- Modify: `CMakeLists.txt`
- Modify: `tests/native_helper_session_test.cpp`

**Meaning:** Native helper starts with an auth session, not a password. This is the main privilege boundary change.

- [ ] **Step 1: Add JSON codec**

`native_auth_session_json.hpp`:

```cpp
#pragma once

#include "vpn_engine/protocol/native_authenticator.hpp"
#include <nlohmann/json.hpp>

namespace ecnuvpn::vpn_engine {

nlohmann::json auth_session_to_json(
    const protocol::NativeAuthSession &session);
bool auth_session_from_json(const nlohmann::json &j,
                            protocol::NativeAuthSession *session);

} // namespace ecnuvpn::vpn_engine
```

The codec must not serialize plaintext password. It may serialize `cookie_header` because helper needs it, but logs and persisted status must redact it.

- [ ] **Step 2: Add helper request mode**

Extend `handle_start()` parsing:

```cpp
std::string native_start_mode =
    request.value("native_start_mode", std::string("password"));
```

Modes:
- `"password"`: existing behavior for legacy compatibility.
- `"auth_session"`: requires `auth_session` object and ignores `password`.

- [ ] **Step 3: Add VPN entry point**

In `vpn.hpp`:

```cpp
int start_native_with_auth_session(
    const Config &cfg,
    const vpn_engine::protocol::NativeAuthSession &auth_session,
    int retry_limit);
```

In `vpn.cpp`, this path spawns the native supervisor with `auth_session` instead of password.

- [ ] **Step 4: Update supervisor payload**

Windows `supervisor_main()` currently reads `"password"`. Add:

```cpp
std::string native_start_mode =
    request.value("native_start_mode", std::string("password"));
if (native_start_mode == "auth_session") {
  protocol::NativeAuthSession auth_session;
  if (!vpn_engine::auth_session_from_json(request.at("auth_session"),
                                          &auth_session)) {
    vpn_engine::persist_native_session_failure(
        config_dir, "invalid_request", "Invalid native auth session payload.");
    return 1;
  }
  return run_native_supervisor_authenticated(cfg, auth_session, retry_limit);
}
```

- [ ] **Step 5: Add helper tests**

`native_helper_session_test.cpp`:
- JSON codec rejects missing `cookie_header`;
- JSON codec round-trips server/useragent/cookie/created_at;
- helper/native mapping does not log cookie value;
- auth-session mode ignores password field.

- [ ] **Step 6: Run tests**

Run:

```powershell
cmake --build build-windows\cpp --target native_helper_session_test
.\build-windows\cpp\native_helper_session_test.exe
```

**Acceptance Criteria:**
- Native helper can start without plaintext password when an auth session is supplied.
- Password remains accepted only for compatibility and direct CLI fallback.

---

### Task 8: Implement Desktop Connect Orchestrator

**Files:**
- Modify: `src/app_api.cpp`
- Modify: `src/platform/common/backend_resolver.hpp`
- Modify: `src/platform/common/backend_resolver.cpp`
- Modify: `src/platform/common/helper_client.hpp`
- Modify: `src/platform/common/helper_client.cpp`
- Modify: `tests/backend_resolver_test.cpp`
- Modify: `tests/app_api_runtime_policy_test.cpp`

**Meaning:** Authentication starts immediately in the desktop/core process. Helper preparation runs in parallel only when it is safe.

- [ ] **Step 1: Add orchestration helper**

In `app_api.cpp`, create a private function:

```cpp
struct NativeConnectPreparation {
  bool helper_available = false;
  nlohmann::json backend;
  std::string error_code;
  std::string error_message;
};

NativeConnectPreparation prepare_native_helper_backend(
    const Config &cfg,
    bool allow_direct_fallback);
```

This function performs helper availability, status fallback, oneshot backend resolution, and service helper resolution. It must not send a `"start"` action.

- [ ] **Step 2: Run auth in a future**

For `cfg.vpn_engine == "native"`:

```cpp
auto auth_future = std::async(std::launch::async, [&] {
  vpn_engine::NativeVpnEngineDependencies deps =
      vpn_engine::default_native_engine_dependencies();
  std::unique_ptr<vpn_engine::protocol::ProtocolTransport> transport =
      deps.transport_factory();
  vpn_engine::protocol::NativeAuthenticator auth(transport.get());
  vpn_engine::protocol::NativeAuthSession session;
  ValidationResult result =
      auth.authenticate({cfg, password, nullptr}, &session);
  return std::pair<ValidationResult,
                   vpn_engine::protocol::NativeAuthSession>{result, session};
});
```

Guard `deps.transport_factory` before calling it and return `native_transport_unimplemented`
when the factory is absent or returns null.

- [ ] **Step 3: Prepare helper safely in parallel**

Service available:
- run `prepare_native_helper_backend()` while auth is pending.
- do not create interfaces or spawn native supervisor.

Oneshot/elevated fallback:
- resolve existing status in parallel;
- do not trigger UAC/oneshot helper start until auth succeeds.

- [ ] **Step 4: Return auth failures immediately**

After helper preparation and auth future complete or auth completes first:

```cpp
if (!auth_result.ok) {
  timing.finish(false, "stage=native_auth reason=" + auth_result.code);
  return error(auth_result.message, auth_result.code);
}
```

If helper preparation is still pending and auth fails, cancel only if the preparation has a cancellation path; otherwise wait for non-mutating preparation to finish and return the auth error.

- [ ] **Step 5: Start helper with auth session**

Build helper request:

```cpp
nlohmann::json request{{"action", "start"},
                       {"config", cfg},
                       {"retry_limit", desktop_retry_limit(cfg)},
                       {"native_start_mode", "auth_session"},
                       {"auth_session",
                        vpn_engine::auth_session_to_json(auth_session)},
                       {"home", utils::get_effective_home()},
                       {"config_dir", utils::get_config_dir()}};
```

No `"password"` field in native auth-session mode.

- [ ] **Step 6: Preserve legacy path**

For `cfg.vpn_engine != "native"`, keep the existing OpenConnect/helper path unchanged.

- [ ] **Step 7: Add tests**

`backend_resolver_test.cpp`:
- helper preparation does not call start action.

`app_api_runtime_policy_test.cpp` or a new app API test:
- native auth failure returns before helper start request;
- native auth success sends helper start with `native_start_mode=auth_session`;
- oneshot fallback does not start UAC helper before auth success.

**Acceptance Criteria:**
- For native mode, bad password/protocol mismatch returns without helper worker bootstrapping.
- Installed helper preparation can overlap with auth.
- One-shot UAC is delayed until auth succeeds unless a future explicit setting chooses otherwise.

---

### Task 9: Reconnect and Reauthentication Semantics

**Files:**
- Modify: `src/vpn.cpp`
- Modify: `src/vpn_engine/native_error_contract.hpp`
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `src/vpn_engine/session_state.hpp`
- Modify: `src/vpn_engine/session_state.cpp`
- Modify: `tests/native_protocol_session_test.cpp`
- Modify: `tests/native_helper_session_test.cpp`

**Meaning:** Once helper no longer owns the password, auto-reconnect must use cookie-only retry and stop cleanly when the gateway demands new auth.

- [ ] **Step 1: Define retryable cookie failures**

Classify:
- `transport_closed`, `tls_handshake_failed`, `network_unreachable`, `no_data`: retryable within retry policy.
- `auth_required`, `auth_cookie_missing`, `auth_protocol_mismatch`, `auth_failed`, `reauth_required`: fatal for supervisor reconnect.

- [ ] **Step 2: Add `reauth_required` mapping**

When CSTP CONNECT returns 401/403 with an auth/login response shape, map to `reauth_required`, not `auth_failed`.

- [ ] **Step 3: Update supervisor loop**

Authenticated supervisor retries by reusing `NativeAuthSession.cookie_header`. It must not call user-mode auth and must not request a password.

- [ ] **Step 4: Add session state**

Persist failure code `reauth_required` and last event `session.reauth_required`.

- [ ] **Step 5: Add tests**

Tests:
- transient CSTP failure retries;
- 401/403 auth rejection stops after one attempt with `reauth_required`;
- no plaintext password exists in supervisor request payload in auth-session mode.

**Acceptance Criteria:**
- Auto-reconnect still works for network blips.
- Expired cookies stop cleanly and ask the desktop/user to reauthenticate.
- No reconnect flood occurs on credential/protocol/session-token failures.

---

### Task 10: UI Feedback and Progress States

**Files:**
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/pages/DashboardPage.vue`
- Modify: `webui/src/components/ErrorDialog.vue`
- Modify: `webui/desktop/shared/desktop-contract.ts`
- Modify: `webui/src/types/ecnu-vpn.d.ts`

**Meaning:** Users see fast, accurate stages: authenticating, preparing helper, starting tunnel, applying routes.

- [ ] **Step 1: Add progress stages**

Add stages:
- `native_authenticating`
- `helper_preparing`
- `helper_starting`
- `cstp_connecting`
- `network_configuring`

- [ ] **Step 2: Map new errors**

Add UI copy:
- `auth_protocol_mismatch`: protocol/gateway mismatch; action `view_logs`.
- `reauth_required`: login expired; action `retry_password`.

- [ ] **Step 3: Avoid password retry for protocol mismatch**

Update:

```ts
if (normalized.error_type === 'auth_failed' ||
    normalized.error_type === 'reauth_required') {
  lastFailedConnectMode.value = 'helper'
}
```

Do not include `auth_protocol_mismatch`.

- [ ] **Step 4: Verify renderer types**

Run:

```powershell
cd webui
npm run typecheck
```

**Acceptance Criteria:**
- Protocol mismatch no longer opens a wrong-password prompt.
- Reauth-required opens the password prompt.
- Connect progress reflects auth before privileged tunnel start.

---

### Task 11: Real Gateway Validation Script

**Files:**
- Create: `scripts/validate-native-auth-once.ps1`
- Modify: `docs/validation/native-production-release-readiness-2026-05-31.md` or create a new `docs/validation/native-auth-helper-decoupling-2026-06-01.md`

**Meaning:** Make the ECNU live validation reproducible and safe: one auth attempt, no flood, redacted diagnostics.

- [ ] **Step 1: Add script**

Script behavior:
- clear old native session state;
- start one native connect with `retry_limit=0`;
- tail last 200 log lines;
- extract `auth.request`, `auth.failed`, `http_status`, `content_type`, `content_length`, `transfer_encoding`, `body_bytes`, and `body_prefix`;
- fail if more than one `auth.started` occurs.

- [ ] **Step 2: Add validation document**

Document exact run:

```powershell
.\scripts\validate-native-auth-once.ps1
```

Expected good failure while protocol is still mismatched:
- exactly one `auth.started`;
- no reconnect attempt;
- error code is `auth_protocol_mismatch`;
- response summary includes enough headers to decide H1 vs H2.

Expected success after protocol fix:
- `auth.succeeded`;
- `cstp.connected`;
- `packet.loop.started`;
- network-ready state true.

**Acceptance Criteria:**
- A live test cannot accidentally produce thousands of reconnects.
- The output is enough for another agent to identify gateway expectations without raw packet capture.

---

### Task 12: Remove Password From Native Helper Path

**Files:**
- Modify: `src/app_api.cpp`
- Modify: `src/helper.cpp`
- Modify: `src/vpn.cpp`
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `docs/security/native-openconnect-replacement-review-2026-05-31.md`
- Modify: `tests/native_helper_session_test.cpp`

**Meaning:** After auth-session mode is working, make it the only desktop native path.

- [ ] **Step 1: Make native desktop path require auth-session mode**

If desktop sends native `"start"` with a plaintext password, return:

```json
{
  "ok": false,
  "code": "invalid_request",
  "message": "Native helper start requires an authenticated session."
}
```

Keep CLI direct `exv start` compatibility until a CLI orchestrator is added.

- [ ] **Step 2: Scrub worker request files**

Assert native auth-session request files do not contain:
- `"password"`;
- raw username/password XML;
- `form-urlencoded` password.

They may contain `"cookie_header"` only inside `auth_session`; logs must redact it.

- [ ] **Step 3: Security review update**

Document:
- password lifetime is confined to desktop/core auth future;
- helper no longer receives plaintext password in desktop native path;
- auth cookie is still a secret and is passed only through helper request file with existing file-permission controls.

**Acceptance Criteria:**
- Native helper worker logs and request payloads contain no plaintext password.
- Desktop native connection still succeeds with auth-session mode.

---

## Dependency and Parallelism Analysis

### Current Gating Order

Use this status for remaining multi-agent work:

1. Completed base: Task 6A authenticated CSTP bootstrap primitive, Task 6B `NativeAuthSession` JSON codec, and Task 6B2 JSON parser header-bound field hardening are accepted.
2. Completed engine/supervisor boundary: Task 6C1 engine `start_authenticated`, Task 6C2a common supervisor payload codec, and Task 6C2b platform supervisor payload wiring are accepted.
3. Completed helper security/mode boundary: Task 6C3a Windows request-file ACL hardening and Task 6C3b helper `native_start_mode=auth_session` are accepted.
4. Completed app API and desktop wiring: Task 6D1 app_api auth-first seam, Task 6D2 real app_api auth-first wiring, and Task 6D3 desktop `connectElevated` normal RPC fallback are accepted.
5. Later service-only prepare optimization: Task 6C3d non-mutating helper prepare/cancel parallelism can be implemented after the auth-first helper start path is live-validated.
6. Reconnect/reauth: Task 9 defines cookie-only retry and `reauth_required`; adopted-auth reconnect is currently disabled until that future design.
7. UI/security/final cleanup: Tasks 10-12 make the behavior visible, redacted, validated, and mandatory for desktop native mode.

Current product behavior is auth-first for native desktop fallback: user-mode auth happens before helper start, auth failures return before helper start, and successful auth sends helper `native_start_mode=auth_session`. The service path intentionally checks helper availability before auth so an unavailable service fails before prompting/authenticating. Do not remove desktop password fallback before Task 9 defines reauth and retry behavior.

### Sequential Dependencies

1. Task 1 is complete and must remain the prerequisite for live ECNU retesting because diagnostics distinguish empty body from chunked body.
2. Task 2 and Task 3 are complete and remain the baseline before investigating new gateway XML variants.
3. Task 4 is complete and remains the prerequisite for UI validation because protocol errors must not look like bad passwords.
4. Task 5 is complete and is the input to Task 6 because `NativeAuthSession` is the authenticated startup descriptor.
5. Task 6A is complete and preceded 6C1 because the engine-only API depends on the authenticated CSTP bootstrap primitive.
6. Task 6B is complete and preceded 6C2a/6C2b because supervisor/helper payloads require a stable codec, exact `schema_version` validation, and secret-safe diagnostics.
7. Task 6B2 is complete and preceded helper/app use of header-bound fields because those fields must be hardened before crossing process boundaries.
8. Task 6C1 is complete and preceded 6C2a/6C2b because the supervisor consumes an engine API that already starts from an authenticated session.
9. Task 6C2a is complete and preceded 6C2b because platform wiring depends on the common payload codec.
10. Task 6C2b is complete and preceded 6C3a/6C3b because request-file security and helper mode needed the platform payload shape.
11. Task 6C3a is complete and preceded app_api wiring because desktop relies on cookie-bearing request files.
12. Task 6C3b is complete and preceded app_api wiring because app_api needed a stable helper `native_start_mode=auth_session` contract.
13. Task 6D1 is complete and preceded 6D2 because the app_api auth-first seam was needed before real product wiring.
14. Task 6D2 is complete and preceded 6D3 because desktop fallback needed the app_api auth-session product path.
15. Task 6C3d service-only helper prepare parallelism remains future work and should not be confused with the completed auth-first connect path.
16. Task 9 depends on helper auth-session mode because reconnect semantics change only after helper no longer owns credentials.
17. Task 12 depends on app API auth-first orchestration and Task 9 because removing password from helper before reconnect semantics exist would break native auto-reconnect.

### Parallel Work Streams

- **Agent A - Protocol/Wire:** Tasks 1, 2, 3, and 6A are complete. Residual parser tests can continue in parallel if they avoid changing the accepted authenticated bootstrap API.
- **Agent B - Error Contract/UI:** Task 4 is complete; Task 10 WebUI type-contract and progress-state work can run in parallel once code names stay frozen.
- **Agent C - Auth Extraction/Codec:** Tasks 5, 6B, and 6B2 are complete. Follow-up changes to `NativeAuthenticator` or `NativeAuthSession` must coordinate with 6C owners because the JSON schema and header-bound fields are now shared contracts.
- **Agent D - Engine/Helper Boundary:** Tasks 6C1, 6C2a, 6C2b, 6C3a, and 6C3b are complete. Helper now accepts `native_start_mode=auth_session` and starts authenticated tunnels from supplied sessions.
- **Agent E - Desktop Orchestration:** Tasks 6D1, 6D2, and 6D3 are complete. Native desktop fallback authenticates before helper start, and `connectElevated` uses normal RPC with `allow_direct_fallback`.
- **Agent F - Reconnect/Security:** Task 9, Task 11 docs/script, and Task 12 security review can proceed using the completed helper auth-session mode and app_api auth-first wiring.

### Explicit Parallel-Safe Work

- Documentation updates and validation docs can proceed anytime.
- WebUI TypeScript contract additions for `auth_protocol_mismatch` and `reauth_required` can proceed after Task 4 and do not need Task 6.
- Parser residual tests for XML/chunked/error cases can proceed if they avoid changing the `NativeAuthSession` struct.
- Security review and secret-redaction tests can proceed using the completed Task 6B JSON shape, 6B2 header-bound field hardening, 6C2 payload redaction behavior, 6C3 request-file hardening, and 6D app_api auth-first path.
- App API follow-up tests can proceed around handle_action E2E coverage, preload test framework gaps, and service-only prepare/cancel behavior.

### Explicit Sequential Work

- Completed protocol authenticated primitive and JSON codec -> completed 6B2 parser hardening -> completed 6C1 engine-only `start_authenticated` -> completed 6C2a common supervisor payload codec -> completed 6C2b platform supervisor wiring -> completed 6C3a request-file ACL hardening -> completed 6C3b helper `native_start_mode=auth_session` -> completed 6D1 app_api auth-first seam -> completed 6D2 real app_api auth-first wiring -> completed 6D3 desktop normal RPC fallback is the executed order.
- Desktop native plaintext-password removal must wait until authenticated helper start and reconnect/reauth behavior are both tested.
- Full helper prepare/cancel parallelism is explicitly out of the current critical path; implement service-only non-mutating prepare/cancel later as Task 6C3d.

### Shared Interfaces To Freeze Early

Freeze these names before parallel implementation:

```cpp
struct NativeAuthSession {
  ParsedVpnUrl server;
  std::string username;
  std::string useragent;
  std::string cookie_header;
  std::string selected_group;
  std::string auth_method;
  std::chrono::system_clock::time_point created_at;
  std::map<std::string, std::string> diagnostics;
};
```

```json
{
  "action": "start",
  "native_start_mode": "auth_session",
  "auth_session": {
    "schema_version": 1,
    "server": "https://vpn.example.invalid/",
    "username": "student@example.invalid",
    "useragent": "AnyConnect Windows 4.10.05095",
    "cookie_header": "webvpn=...",
    "selected_group": "",
    "auth_method": "password"
  }
}
```

Error codes:
- `auth_failed`: real credential rejection.
- `auth_protocol_mismatch`: gateway response/request shape mismatch.
- `reauth_required`: cookie/session expired after prior success.
- `unsupported_auth_flow`: SAML/browser/cert flow not supported by native engine.
- `auth_challenge_required`: interactive challenge exists but no UI path is available yet.

---

## Risks and Open Questions

- No full `handle_action` E2E test exists yet for the complete desktop/app_api/native helper flow.
- No preload test framework exists yet, so desktop bridge fallback coverage is still limited.
- No true helper prepare/cancel parallelism exists yet; Task 6C3d remains future service-only non-mutating prepare/cancel work.
- Live ECNU gateway validation is still needed for the auth-first helper path.
- Adopted-auth reconnect is currently disabled; cookie-only reconnect/reauth remains future Task 9 work.
- DNS apply remains unresolved and still needs validation beyond successful auth/tunnel startup.
- Cookie TTL/session ID: ECNU may bind cookie/session tokens to the original TLS connection, source address, user agent, or a short TTL. Completed 6C1/6C2a/6C2b/6C3a/6C3b/6D1/6D2/6D3 prove the engine, supervisor, helper, app_api, and desktop fallback path can carry an auth-session descriptor, but live app_api validation must still confirm whether ECNU accepts CSTP bootstrap from the transferred `cookie_header` and explicit metadata.
- Host-scan/2FA/group selection: current extraction handles the password aggregate-auth path first. Host-scan, secondary challenge, browser/SAML, certificate auth, and group-selection variants may require `unsupported_auth_flow` or `auth_challenge_required` until UI orchestration exists.
- Helper socket owner check: before accepting serialized auth sessions over helper IPC, verify the caller ownership/permissions model prevents another local process from injecting or stealing session-bearing requests.
- Cross-process cookie transfer: Task 7 intentionally serializes `cookie_header` because helper needs CSTP bootstrap. Revisit whether cookie can stay in core and be exchanged through a narrower IPC or same-process bootstrap on platforms where that is practical.
- Secret redaction: logs, native session state, helper request files, diagnostics, validation scripts, and UI error payloads must never expose cookie, password, session-id, form password XML, or raw `Set-Cookie` values.
- Packet-loop transition: short term allows helper/supervisor to retain CSTP packet loop ownership. Long term may move more of CSTP/TLS back to user mode, but that is outside this plan unless Task 6C proves helper cannot consume a transferred auth session.

---

## Verification Matrix

Run after each phase:

```powershell
cmake --build build-windows\cpp --target native_production_transport_test native_auth_parser_test native_protocol_session_test native_engine_contract_test native_auth_session_json_test native_helper_session_test feedback_test
.\build-windows\cpp\native_production_transport_test.exe
.\build-windows\cpp\native_auth_parser_test.exe
.\build-windows\cpp\native_protocol_session_test.exe
.\build-windows\cpp\native_engine_contract_test.exe
.\build-windows\cpp\native_auth_session_json_test.exe
.\build-windows\cpp\native_helper_session_test.exe
.\build-windows\cpp\feedback_test.exe
```

Run for completed Task 6B2 parser hardening:

```powershell
cmake --build build-windows\cpp --target native_auth_session_json_test native_auth_parser_test
.\build-windows\cpp\native_auth_session_json_test.exe
.\build-windows\cpp\native_auth_parser_test.exe
```

Run for completed Task 6C1 engine-only work:

```powershell
cmake --build build-windows\cpp --target native_engine_contract_test native_protocol_session_test
.\build-windows\cpp\native_engine_contract_test.exe
.\build-windows\cpp\native_protocol_session_test.exe
```

Run for completed Task 6C2a/6C2b supervisor payload work and any payload/security changes:

```powershell
cmake --build build-windows\cpp --target native_auth_session_json_test native_helper_session_test
.\build-windows\cpp\native_auth_session_json_test.exe
.\build-windows\cpp\native_helper_session_test.exe
```

Run for completed Task 6C3a Windows request-file ACL hardening:

```powershell
cmake --build build-windows\cpp --target native_helper_session_test
.\build-windows\cpp\native_helper_session_test.exe
```

Run for completed Task 6C3b helper auth-session mode:

```powershell
cmake --build build-windows\cpp --target native_auth_session_json_test native_helper_session_test
.\build-windows\cpp\native_auth_session_json_test.exe
.\build-windows\cpp\native_helper_session_test.exe
```

Run for completed Task 6D1/6D2 app API auth-first seam and real wiring:

```powershell
cmake --build build-windows\cpp --target app_api_runtime_policy_test backend_resolver_test
.\build-windows\cpp\app_api_runtime_policy_test.exe
.\build-windows\cpp\backend_resolver_test.exe
```

Run for completed Task 6D3 desktop `connectElevated` normal RPC fallback:

```powershell
cd webui
npm run typecheck
```

Run for Task 6C3d service-only non-mutating prepare/cancel:

```powershell
cmake --build build-windows\cpp --target app_api_runtime_policy_test backend_resolver_test
.\build-windows\cpp\app_api_runtime_policy_test.exe
.\build-windows\cpp\backend_resolver_test.exe
```

Run after UI contract changes:

```powershell
cd webui
npm run typecheck
```

Run before staging:

```powershell
.\scripts\validate-merge-prep-windows.ps1
.\scripts\stage-openconnect-runtime-win.ps1
```

Manual live ECNU validation:
- one connect attempt with bad password returns `auth_failed`;
- one connect attempt with protocol mismatch returns `auth_protocol_mismatch`;
- one native desktop fallback connect with valid auth performs user-mode auth before helper start;
- one service-path connect checks helper availability before auth and returns auth failures before helper start;
- one successful service-path connect sends helper `native_start_mode=auth_session` and the helper starts the authenticated tunnel;
- no run emits more than one `auth.started` unless the user explicitly retries.

Final follow-up checklist:
- full `handle_action` E2E coverage still needed;
- preload test framework still needed;
- true helper prepare/cancel parallelism still needed;
- live ECNU gateway validation still needed;
- adopted-auth reconnect/reauth design still needed;
- DNS apply validation still unresolved.

---

## Completion Criteria

- Correct password no longer appears as wrong password unless the gateway returned a credential rejection.
- `auth_protocol_mismatch` remains visible through native engine, helper, app API, and renderer.
- XML aggregate-auth POSTs use the correct content type and parse chunked XML responses.
- Desktop native mode authenticates in user space before privileged network setup.
- Service path checks helper availability before auth; one-shot fallback waits until auth succeeds before helper/UAC by default.
- Helper/supervisor native path can start from `NativeAuthSession` without plaintext password.
- Reconnect uses existing auth session for transient CSTP failures and stops with `reauth_required` when session auth expires.
- Windows and macOS native test suites pass, and the staged `exv.exe` contains the new behavior.
