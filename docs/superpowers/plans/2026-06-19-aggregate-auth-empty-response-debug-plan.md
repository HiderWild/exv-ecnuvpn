# Aggregate-Auth Empty Response Debug Plan

> **For agentic workers:** REQUIRED SUB-SKILLS: `superpowers:systematic-debugging`, `superpowers:test-driven-development`. Steps use checkbox (`- [ ]`) syntax for tracking. **Iron law:** no fix without root-cause evidence; no production code without a failing test first.

**Goal:** Locate the precise layer that turns a real ECNU AnyConnect aggregate-auth handshake into the user-visible failure `auth_response_invalid: "aggregate auth response is empty"`, prove the cause with reproducible evidence, then fix the root cause without re-introducing the bug.

**Symptom (user-reported):** After the connect-pipeline concurrency refactor (`2026-06-19-core-rpc-lane-isolation-and-connect-pipeline-concurrency-plan.md` Tasks 1–13), every real connect attempt — both with and without the helper service installed — fails with the same envelope:

```
error_code = "auth_response_invalid"
error      = "aggregate auth response is empty"
```

The same code path passes every fake-server test in CI. The verification report explicitly leaves "Real connect against the VPN gateway" as not-yet-executed.

---

## Phase 1 — Root-Cause Investigation (REQUIRED before any fix)

### Background evidence already gathered

| # | Evidence | Source |
|---|---|---|
| E1 | The string `"aggregate auth response is empty"` is generated in exactly one place — `ValidationResult parse_aggregate_auth_response()` when `trim(xml).empty()` | `src/vpn_engine/protocol/aggregate_auth.cpp:386-387` |
| E2 | This parser is called from three sites in `production_transport.cpp` (init / submit-reply / followup) | `src/vpn_engine/protocol/production_transport.cpp:597, 694, 770` |
| E3 | The parser is invoked **after** the HTTP status check passes (status in `[200,300)`) | `production_transport.cpp:586-591, 683-688` |
| E4 | New connect pipeline runs aggregate-auth exactly once, in the `protocol_handshake` branch via `NativeHandshakeJob::run()` → `ProtocolSession::authenticate()` | `src/core/app_api/desktop_vpn_actions.cpp:442-501` + `src/vpn_engine/native_handshake_job.cpp:88-191` |
| E5 | `do_connect()` adopts the prepared session via `runner_.start_from_handshake()` → `adopt_handshake()` and **does not** re-authenticate | `src/core/tunnel_controller/tunnel_controller_connect.cpp:385-392` + `src/vpn_engine/native_engine.cpp:178-223` + `src/core/tunnel_controller/core_session_runner.cpp:135-221` |
| E6 | Win32 default deps inject a real `NativeTlsStream` + `ProductionProtocolTransport`; no fake transport is in the production path | `src/platform/win32/default_engine_deps.cpp:11-24` |
| E7 | `read_http_response()` returns OK with an empty body if `Content-Length: 0` is present, and silently truncates body to whatever is already buffered if Content-Length is absent and the server uses chunked / close-delimited framing | `src/vpn_engine/protocol/production_transport.cpp:1075-1125` |
| E8 | In `desktop_vpn_actions.cpp:459-461` the pipeline-branch `NativeHandshakeJob` is constructed with `default_native_engine_dependencies()` and **no `event_sink`**, so the in-flight `auth.started` / `auth.failed` / `cstp.failed` events emitted by the handshake are dropped — current logs cannot tell us which sub-step failed | `src/core/app_api/desktop_vpn_actions.cpp:459-461` |
| E9 | `core_rpc_lane_isolation` plan verification report explicitly states "Real connect against the VPN gateway" was *not* executed | `docs/superpowers/reports/2026-06-19-core-rpc-lane-isolation-and-connect-pipeline-concurrency-verification.md:124` |

### Bounded conclusion from evidence alone

- The failure cannot be a helper / RPC / lane-scheduler issue — those layers are not on the call path between TLS bytes and `parse_aggregate_auth_response()`.
- The failure cannot be a "real authentication rejection" — a real reject returns `<error>...</error>` XML mapped to `auth_rejected`, not `auth_response_invalid`.
- The failure must be one of:
  - **A.** A real HTTP body-of-zero-bytes coming back from the gateway (server actually returned 0 bytes after a status line that wasn't 4xx/5xx).
  - **B.** A protocol/parsing bug where bytes did arrive but `read_http_response()` gave us an empty `body` field anyway.
  - **C.** A connection-state bug where the new pipeline writes the auth-init request before TLS / SNI / cookies are properly set up, and the gateway answers with an empty success-shaped page.

The only way to choose between A/B/C is **wire-level evidence**, not more reading.

### Three competing hypotheses

| H | Layer | Prediction if true | Evidence we need to confirm/refute |
|---|---|---|---|
| **H1** *Server returns empty 200* (e.g. `Content-Length: 0`, or 200 OK with HTML-empty body, due to malformed first request: missing User-Agent header, no `Cookie:`, wrong path, wrong `Content-Type`) | request layer (`make_aggregate_auth_post_request` / `build_aggregate_auth_init_xml`) | Raw response header has `Content-Length: 0` *or* status 200 with literally empty body | Header dump of the first auth-init response |
| **H2** *Body bytes arrive but framing is mishandled* — server uses `Transfer-Encoding: chunked` or `Connection: close` framing; `read_http_response()` returns OK with truncated/empty body (see `production_transport.cpp:1114-1125`) | HTTP read layer | Raw socket bytes contain a non-empty body, but `init_http.body` after `read_http_response()` is empty | Hex dump of (a) raw bytes after TLS decrypt and (b) `init_http.body` |
| **H3** *TLS / SNI / connection-state regression* — the new pipeline opens a TLS connection but with wrong SNI / wrong port / wrong host, server answers with default index page or empty | TLS layer (`NativeTlsStream`) | TLS connect succeeds but server returns content not matching aggregate-auth | TLS handshake log + raw response first 200 bytes |

The plan below probes all three in parallel with one diagnostic build, then commits the fix only after the evidence picks one hypothesis.

### Out of scope

- Any change to the lane scheduler, RPC dispatch, or webview shell code.
- Any change to helper IPC or backend resolver.
- Any addition or removal of branches in `ConnectPipeline`.
- The known unrelated full-ctest 0xc0000139 failures (per project memory `release-blocking-gate`).

---

## Task 1 — Capture the failure deterministically

**Files:**
- Create: `docs/superpowers/debug/2026-06-19-aggregate-auth-empty-response/observation.md`
- Create: `docs/superpowers/debug/2026-06-19-aggregate-auth-empty-response/raw-init-response.bin` (binary, redacted, captured by Step 3)

- [ ] **Step 1: Reproduce on the developer machine**

Run the packaged build against the real ECNU gateway:

```powershell
cmake --build --preset windows-release --target exv exv-cli exv-helper
./build-windows/cpp/exv-cli.exe vpn.connect "{\"password\":\"<real-password>\"}"
```

Confirm the response envelope contains:

```json
{"ok": false, "error_code": "auth_response_invalid",
 "error": "aggregate auth response is empty"}
```

Record exit code, timestamp, and contents of `%APPDATA%/ecnuvpn/ecnuvpn.log` between the click and the failure into `observation.md`. Do **not** commit the password or any cookie values.

- [ ] **Step 2: Confirm both helper modes hit the same failure**

Repeat Step 1 with helper service uninstalled (`exv-cli service uninstall` first) and with helper service installed and started. Both should produce the identical error envelope. Record both runs in `observation.md`. If they diverge, the root-cause hypotheses change — stop and restart Phase 1.

- [ ] **Step 3: Capture the raw first-response bytes**

Add a temporary diagnostic block (gate behind an env var so it never ships) inside `ProductionProtocolTransport::auth()` immediately after the first `read_http_response(false, &init_http)` returns OK and before `parse_aggregate_auth_response()` is called:

```cpp
if (const char* dump_path = std::getenv("EXV_DEBUG_AUTH_INIT_DUMP")) {
  std::ofstream dump(dump_path, std::ios::binary | std::ios::trunc);
  dump << "-- status=" << init_http.status << "\n";
  dump << "-- header_count=" << init_http.headers.size() << "\n";
  for (auto& h : init_http.headers) {
    dump << h.first << ": " << h.second << "\n";   // do NOT dump Set-Cookie values
  }
  dump << "-- body_size=" << init_http.body.size() << "\n";
  dump.write(init_http.body.data(),
             static_cast<std::streamsize>(init_http.body.size()));
}
```

Run the failing connect again with `EXV_DEBUG_AUTH_INIT_DUMP=docs/superpowers/debug/2026-06-19-aggregate-auth-empty-response/raw-init-response.bin`. Inspect the file:

```powershell
Get-Content -Encoding Byte raw-init-response.bin | Format-Hex | Select-Object -First 60
```

Record in `observation.md`:
- HTTP status code
- Whether `Content-Length` header is present and its value
- Whether `Transfer-Encoding: chunked` is present
- Whether `Connection: close` is present
- The literal `body_size`
- The first 200 hex bytes of the raw body

Redact any `Set-Cookie` values before commit.

- [ ] **Step 4: Decide which hypothesis matches**

Apply this decision matrix to the captured evidence:

| Observation | Verdict | Next task |
|---|---|---|
| Status 200, `Content-Length: 0`, body empty | **H1 confirmed** — server got something it didn't like in the request | Task 3 |
| Status 200, `Content-Length: <large>`, `body_size == 0` | **H2 confirmed** — read pipeline truncated body | Task 4 |
| Status 200, no `Content-Length`, has `Transfer-Encoding: chunked`, body empty or truncated | **H2 confirmed** | Task 4 |
| Status 200, body looks like HTML (contains `<html`) | partial **H1** — first request landed on the wrong endpoint; should already be caught as `auth_protocol_mismatch`, so framing is ALSO wrong | Task 4 then Task 3 |
| Status 302 / 401 / other | **None of the above** — re-examine wire-level captures and stop | Phase 1 restart |
| Status 200, body is a valid `<config-auth>` XML | **Bug not reproduced** — ask the user to reproduce | Phase 1 restart |

- [ ] **Step 5: Commit the observation, NOT the diagnostic block**

```powershell
git add docs/superpowers/debug/2026-06-19-aggregate-auth-empty-response/observation.md docs/superpowers/debug/2026-06-19-aggregate-auth-empty-response/raw-init-response.bin
git commit -m "debug: capture aggregate-auth empty-response observation"
```

Revert the temporary diagnostic block (Task 2 will reintroduce it as a real, gated facility).

---

## Task 2 — Restore handshake observability (RED → GREEN, regardless of which hypothesis wins)

**Files:**
- Modify: `src/core/app_api/desktop_vpn_actions.cpp`
- Modify: `src/vpn_engine/native_handshake_job.cpp`
- Create: `tests/desktop_connect_handshake_observability_test.cpp`
- Modify: `CMakeLists.txt`

Rationale: even after the immediate fix, we should never again be unable to tell from logs which sub-step of `NativeHandshakeJob::run()` failed. The handshake job already emits `auth.started`, `auth.failed`, `cstp.failed`, etc. (`native_handshake_job.cpp:128-167`), but `desktop_vpn_actions.cpp:459` builds the job with `default_native_engine_dependencies()` and never wires `event_sink`, so they go nowhere.

- [ ] **Step 1: RED test asserting handshake events reach the desktop log facade**

Create `tests/desktop_connect_handshake_observability_test.cpp` that:

1. Installs a fake `transport_factory` whose `auth()` returns `auth.code = "auth_response_invalid"` with message `"aggregate auth response is empty"`.
2. Drives `run_desktop_connect_job(...)` (extract a thin test seam if necessary; do NOT use real network).
3. Captures `LogFacade` output via an injected sink.
4. Asserts the captured log contains `auth.started`, `auth.failed`, AND the canonical error code/message.

Run the test:

```powershell
cmake --build --preset windows-release --target desktop_connect_handshake_observability_test
ctest --test-dir build-windows/cpp -R desktop_connect_handshake_observability_test --output-on-failure
```

Confirm it FAILS because nothing routes the events.

- [ ] **Step 2: GREEN — wire the sink**

In `desktop_vpn_actions.cpp` at the construction of the handshake `deps` (around line 459):

```cpp
auto deps = ecnuvpn::vpn_engine::default_native_engine_dependencies();
struct DesktopHandshakeEventSink : ecnuvpn::vpn_engine::EventSink {
  void emit(const ecnuvpn::vpn_engine::VpnEngineEvent& event) override {
    nlohmann::json fields = event.fields;
    exv::observability::LogFacade::info(
        "native.handshake event=" + event.type +
        " level=" + event.level +
        " message=" + sanitize_for_log(event.message) +
        " fields=" + fields.dump());
  }
};
static DesktopHandshakeEventSink sink;
deps.event_sink = &sink;
```

`sanitize_for_log` must reuse the same redaction the existing protocol layer uses (`sanitized_result()`); never log password, cookie, opaque, or session-token values. Add an assertion to the RED test that the `EXV_VPN_PASSWORD`-looking string never appears in the captured log.

- [ ] **Step 3: Verify**

```powershell
cmake --build --preset windows-release --target desktop_connect_handshake_observability_test no_secret_in_logs_test
ctest --test-dir build-windows/cpp -R "desktop_connect_handshake_observability_test|no_secret_in_logs_test" --output-on-failure
```

- [ ] **Step 4: Commit**

```powershell
git add src/core/app_api/desktop_vpn_actions.cpp tests/desktop_connect_handshake_observability_test.cpp CMakeLists.txt
git commit -m "core: route native handshake events to desktop log facade"
```

---

## Task 3 — Fix path: H1 confirmed (server got something wrong in our request)

Skip if Task 1 Step 4 picked H2 only.

**Files:**
- Modify: `src/vpn_engine/protocol/production_transport.cpp` (auth-init request construction only)
- Modify: `src/vpn_engine/protocol/aggregate_auth.cpp` if `build_aggregate_auth_init_xml()` is missing fields
- Modify: `tests/native_aggregate_auth_test.cpp`
- Modify: `tests/native_production_transport_test.cpp`
- Add: `tests/fixtures/native_anyconnect_v2/auth_init_request_real_ecnu.txt` (request bytes the server accepts; redacted)

- [ ] **Step 1: Compare the failing request against a known-good request**

The known-good request is whatever the previous (pre-pipeline) code produced. Reconstruct it by checking out the parent of the connect-pipeline merge and running the same `EXV_DEBUG_AUTH_INIT_DUMP` recipe (Task 1 Step 3) against the same gateway, this time dumping the **request** as well — temporarily log the bytes returned by `make_aggregate_auth_post_request(...)` *before* `stream_->write_all()`. Save as `tests/fixtures/native_anyconnect_v2/auth_init_request_real_ecnu.txt` after redacting cookies / hostnames.

If git history does not have a known-good commit (the bug predates the pipeline refactor), generate a known-good request from a packet capture of the **vendor** AnyConnect client against the same gateway and use that as the reference.

- [ ] **Step 2: RED test — capture the difference as a contract test**

Add a test in `tests/native_production_transport_test.cpp` that:

1. Builds the auth-init request with the current code.
2. Asserts every header in the reference fixture is present with the same value (case-insensitive header name compare).
3. Asserts the body XML produced by `build_aggregate_auth_init_xml()` matches the reference XML modulo whitespace.

Run; expect failure on whichever specific header / element is wrong.

- [ ] **Step 3: GREEN — restore the missing field(s)**

Apply the smallest possible change to make the test pass. Do NOT bundle other improvements. Likely candidates (do not assume; prove with the diff):

- `User-Agent` header value (must match `useragent_or_default(useragent_)` and be the AnyConnect-compatible string the gateway whitelists)
- `Content-Type` header
- `X-Aggregate-Auth` / `X-Transcend-Version` header
- Initial `Cookie:` header from a prior `GET /` if the gateway's CSD pre-handshake sets one
- The `<config-auth>` element's `client="vpn"` / `type="init"` attributes
- The `<device-id>` and `<version>` fields

- [ ] **Step 4: Verify locally against real gateway**

Run the failing connect again from Task 1. Now expect either success or a different, semantically meaningful error (e.g. `auth_rejected` if password is wrong). The exact error `"aggregate auth response is empty"` must not reproduce.

```powershell
./build-windows/cpp/exv-cli.exe vpn.connect "{\"password\":\"<real-password>\"}"
```

Update `observation.md` with the new outcome.

- [ ] **Step 5: Run release-blocking suite**

```powershell
cmake --build --preset windows-release --target native_aggregate_auth_test native_production_transport_test connect_pipeline_test vpn_actions_test core_session_runner_test desktop_connect_handshake_observability_test
ctest --test-dir build-windows/cpp -R "native_aggregate_auth_test|native_production_transport_test|connect_pipeline_test|vpn_actions_test|core_session_runner_test|desktop_connect_handshake_observability_test" --output-on-failure
./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking
```

Per `release-blocking-gate` project memory: the release-blocking suite is the gate; isolated 0xc0000139 failures elsewhere stay out of scope.

- [ ] **Step 6: Commit**

```powershell
git add src/vpn_engine/protocol/production_transport.cpp src/vpn_engine/protocol/aggregate_auth.cpp tests/native_aggregate_auth_test.cpp tests/native_production_transport_test.cpp tests/fixtures/native_anyconnect_v2/auth_init_request_real_ecnu.txt
git commit -m "fix: restore aggregate-auth init request shape expected by ecnu gateway"
```

---

## Task 4 — Fix path: H2 confirmed (HTTP read layer truncates body)

Skip if Task 1 Step 4 picked H1 only.

**Files:**
- Modify: `src/vpn_engine/protocol/production_transport.cpp` (`read_http_response()`)
- Modify: `tests/native_production_transport_test.cpp`
- Add (if missing): `tests/fixtures/native_anyconnect_v2/auth_init_response_chunked.bin`
- Add (if missing): `tests/fixtures/native_anyconnect_v2/auth_init_response_close_delimited.bin`
- Add (if missing): `tests/fixtures/native_anyconnect_v2/auth_init_response_content_length_zero.bin`

- [ ] **Step 1: RED tests — three response shapes**

In `tests/native_production_transport_test.cpp`, add three test cases each driving `read_http_response()` against a fake `TlsStream` that replays one of the new fixture binary files:

1. `chunked` — server sends `Transfer-Encoding: chunked` with a real `<config-auth>` body.
2. `close-delimited` — server sends body, then closes the connection without `Content-Length` and without chunked.
3. `Content-Length: 0` — server sends explicit zero-length body. This case must surface as a *meaningful* error (`auth_protocol_error` with message containing "empty body, status 200"), **not** the misleading `auth_response_invalid: aggregate auth response is empty`.

Run; expect chunked to fail (current code drops chunks), close-delimited may or may not fail, and Content-Length-zero will fail because the diagnostic message is wrong (or surface as `aggregate auth response is empty`).

- [ ] **Step 2: GREEN — handle chunked and close-delimited**

In `read_http_response()` (`production_transport.cpp:1075-1125`):

1. Detect `Transfer-Encoding: chunked` in `header_response.headers`. If present, call a new helper `read_chunked_body(response)` that:
   - Reads chunk-size lines, then exactly chunk-size bytes, then `\r\n`, until a `0\r\n\r\n` terminator.
   - Enforces `kMaxHttpBodyBytes`.
   - Returns `invalid("http_chunked_truncated", ...)` if EOF before terminator.
2. If `Connection: close` is present and `Content-Length` is absent and `Transfer-Encoding: chunked` is absent, loop `read_more()` until the underlying stream returns EOF, accumulating all bytes as body. Cap at `kMaxHttpBodyBytes`.
3. If `Content-Length: 0` and the request is an aggregate-auth POST, the existing parser will return `auth_response_invalid: empty`. Add an upstream guard at the call sites in `auth()` that converts an HTTP 200 + zero-byte body into `auth_protocol_error: "aggregate-auth init returned empty body, status 200"` so the next operator sees the real cause, not a parse-shape symptom.

- [ ] **Step 3: Verify locally against real gateway**

Repeat the failing connect from Task 1. The empty-body symptom must not reproduce.

- [ ] **Step 4: Run focused + release-blocking suites**

```powershell
cmake --build --preset windows-release --target native_production_transport_test native_aggregate_auth_test connect_pipeline_test vpn_actions_test core_session_runner_test desktop_connect_handshake_observability_test
ctest --test-dir build-windows/cpp -R "native_production_transport_test|native_aggregate_auth_test|connect_pipeline_test|vpn_actions_test|core_session_runner_test|desktop_connect_handshake_observability_test" --output-on-failure
./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking
```

- [ ] **Step 5: Commit**

```powershell
git add src/vpn_engine/protocol/production_transport.cpp tests/native_production_transport_test.cpp tests/fixtures/native_anyconnect_v2/auth_init_response_*.bin
git commit -m "fix: handle chunked and close-delimited http bodies in aggregate-auth read"
```

---

## Task 5 — Final verification and report

**Files:**
- Create: `docs/superpowers/reports/2026-06-19-aggregate-auth-empty-response-fix.md`

- [ ] **Step 1: Run the full focused matrix**

```powershell
cmake --build --preset windows-release --target native_aggregate_auth_test native_production_transport_test connect_pipeline_test connect_intent_test vpn_actions_test core_session_runner_test core_process_lifecycle_test app_api_status_contract_test desktop_connect_handshake_observability_test no_secret_in_logs_test
ctest --test-dir build-windows/cpp -R "native_aggregate_auth_test|native_production_transport_test|connect_pipeline_test|connect_intent_test|vpn_actions_test|core_session_runner_test|core_process_lifecycle_test|app_api_status_contract_test|desktop_connect_handshake_observability_test|no_secret_in_logs_test" --output-on-failure
```

- [ ] **Step 2: Run release-blocking and frontend gates**

```powershell
pnpm --dir webui test:host
pnpm --dir webui exec vue-tsc -b
./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking
```

- [ ] **Step 3: Manual real-gateway acceptance**

Run a fresh real-gateway connect twice (helper-service uninstalled, then installed). Expected:

- Failure case stays correctly mapped: e.g. wrong password produces `auth_rejected`, not `auth_response_invalid`.
- Success case: the tunnel comes up; `status.get` returns `connected=true`.
- Logs contain a contiguous `native.handshake event=auth.started ... auth.succeeded ... cstp.connected` chain.

- [ ] **Step 4: Write the report**

`docs/superpowers/reports/2026-06-19-aggregate-auth-empty-response-fix.md` must include:

- Hypothesis chosen (H1, H2, or both) and the evidence (from `observation.md`) that picked it.
- Final commit hashes for each task.
- Output summary of the focused command matrix.
- Release-blocking pass/fail count.
- Manual real-gateway connect outcome (one line per attempt, redacted).

- [ ] **Step 5: Commit verification report**

```powershell
git add docs/superpowers/reports/2026-06-19-aggregate-auth-empty-response-fix.md
git commit -m "docs: record aggregate-auth empty-response root cause and fix"
```

---

## Self-Review

- **Iron law honored:** No fix is committed without (a) the `observation.md` evidence in Task 1, (b) a RED test in Task 2 / 3 / 4, and (c) explicit GREEN verification.
- **Symptom vs. root cause:** The original failure (`"aggregate auth response is empty"`) was a *symptom* shared by three distinct root causes; the plan probes all three with one diagnostic build before choosing a fix path. No "shotgun" multi-fix attempts.
- **Observability never regresses:** Task 2 stands on its own — even if Task 3/4 picks the wrong hypothesis on first try, every subsequent diagnostic round will have proper handshake-event logs.
- **Architectural debt acknowledged:** Per project memory `release-blocking-gate`, isolated full-ctest 0xc0000139 failures remain out of scope. Per project memory `core-lifecycle-contract-debt`, file relocation is still deferred.
- **Scope discipline:** No changes to lane scheduler, RPC dispatch, helper IPC, webview shell, or backend resolver. The whole plan touches at most six C++ files plus tests/fixtures/docs.
