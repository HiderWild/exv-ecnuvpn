# Native AnyConnect (ECNU) — Clean‑Room Protocol Spec (Phase 2)

**Ownership:** This document is the sole artifact for Task A1.

**Goal:** Provide a clean-room, implementation-ready specification for the v1 native AnyConnect-compatible flow used by ECNU-VPN, without reproducing OpenConnect source or implementation structure.

---

## 1) Scope

### Supported in v1

- **ECNU AnyConnect-compatible username/password authentication** (HTML form login).
- **CSTP over TCP/TLS** (single TLS channel).
- **IPv4 tunnel configuration**:
  - IPv4 tunnel address
  - MTU
  - split routes
  - server bypass route
- **UTF-8 structured events** emitted by the native implementation.

### Unsupported in v1

- DTLS acceleration.
- SAML / browser-based authentication.
- certificate enrollment.
- arbitrary OpenConnect *extra arguments*.
- GlobalProtect, Pulse, Juniper, Fortinet protocols.

---

## 2) Clean-room sources

### Allowed sources

- **Live traces captured from our own ECNU sessions**, with credentials and any user-identifying data redacted.
- **Public protocol descriptions** (RFCs and vendor-neutral writeups).
- **Current app behavior and tests** in this repository.
- OpenConnect **only as a behavioral reference point** (e.g., “legacy client succeeds/fails in scenario X”), with no reproduction of its source, constants blocks, parsing code, state machines, or comments.

### Forbidden sources

- Reproducing any OpenConnect source code, parser/state-machine structure, constant tables, or comments.
- Deriving protocol details by transcribing OpenConnect internals.

---

## 3) Request/response flow (v1)

This section defines:
- **Network steps** (HTTP/TLS/CSTP) at a protocol level.
- **Exact internal field names** for events and intermediate results.

### 3.0 Structured event envelope (UTF-8)

All events are UTF‑8 JSON objects with the following top-level fields:

- `event.type` (string)
- `event.ts_unix_ms` (int)
- `event.level` (string: `debug` | `info` | `warn` | `error`)
- `event.fields` (object; per-event schema below)

The remainder of this spec uses **dot-separated paths** to refer to keys under `event.fields`.

---

### 3.1 Server URL normalization

**Input:** `server.url_raw` (string)

**Output fields:**
- `server.url_normalized` (string)
- `server.scheme` (string: `https`)
- `server.host` (string; no port)
- `server.port` (int; default 443)
- `server.origin` (string: `https://{host}:{port}`; omit `:{port}` when port is 443)

**Normalization rules (v1):**
1. Trim whitespace.
2. If the input has no scheme, treat it as `https://{input}`.
3. Parse as a URL.
4. Reject non-HTTPS schemes.
5. If the URL contains a path/query/fragment, they are ignored for the **base server identity**.
6. `server.host` is the URL hostname (no brackets, no port).
7. `server.port` is the explicit port if provided, else 443.
8. `server.url_normalized` equals `server.origin`.

**Events:**
- `event.type = "server.normalized"` with the output fields above.

---

### 3.2 TLS SNI host

**Field:** `tls.sni_host` (string)

**Rule (v1):**
- `tls.sni_host` MUST equal `server.host`.

**Events:**
- `event.type = "tls.handshake.start"` includes `tls.sni_host`, `server.origin`.
- `event.type = "tls.handshake.ok"` includes `tls.peer_subject` and `tls.peer_san` (strings; if available).

---

### 3.3 Login path

**Field:** `login.path` (string)

**Rule (v1):**
- `login.path` is `"/+CSCOE+/logon.html"`.

**Derived fields:**
- `login.url` (string) = `{server.origin}{login.path}`

**Events:**
- `event.type = "login.path.selected"` includes `login.path`, `login.url`.

---

### 3.4 Login request (preflight)

**Purpose:** obtain initial cookies (and any hidden fields if present).

**HTTP request fields:**
- `http.request.method` = `"GET"`
- `http.request.url` = `login.url`
- `http.request.headers.user_agent` (string)

**HTTP response fields:**
- `http.response.status` (int)
- `http.response.headers.set_cookie` (array of strings; raw header lines)

**Events:**
- `event.type = "http.request"` and `event.type = "http.response"` with the fields above.

---

### 3.5 Username/password submit

**HTTP request:**
- `http.request.method` = `"POST"`
- `http.request.url` = `login.url`
- `http.request.headers.content_type` = `"application/x-www-form-urlencoded; charset=utf-8"`
- `http.request.headers.cookie` = `session.cookie_header`

**Form fields (exact names in v1):**
- `login.form.username_field_name` = `"username"`
- `login.form.password_field_name` = `"password"`

**Encoded body fields:**
- `{login.form.username_field_name}={login.username_utf8_percent_encoded}`
- `{login.form.password_field_name}={login.password_utf8_percent_encoded}`

**Events:**
- `event.type = "auth.submit"` includes `login.form.username_field_name`, `login.form.password_field_name`.

---

### 3.6 Auth failure detection

**Decision output:**
- `auth.result` (string: `success` | `failure`)
- `auth.failure_reason` (string; present when `failure`)

**Rules (v1):**
- If the POST response status is `401` or `403` ⇒ `auth.result = "failure"`.
- Else if no session is established (see §3.7) ⇒ `auth.result = "failure"`.
- Otherwise ⇒ `auth.result = "success"`.

**Events:**
- `event.type = "auth.result"` includes `auth.result` and (if failure) `auth.failure_reason`.

---

### 3.7 Cookie / session extraction

**Input:** all `Set-Cookie` headers observed from §3.4 and §3.5.

**Extracted fields:**
- `session.cookies` (object: cookie-name → cookie-value)
- `session.cookie_header` (string)

**Rules (v1):**
1. Parse each `Set-Cookie` header line.
2. Store the first `name=value` pair into `session.cookies[name] = value`.
3. Build `session.cookie_header` by joining `name=value` pairs with `"; "`, in a stable insertion order.
4. `session.cookie_header` MUST be used as the `Cookie` header for subsequent requests, including CSTP.

**Session established (v1):**
- `session.cookie_header` is considered established if `session.cookies` is non-empty.

**Events:**
- `event.type = "session.cookies.extracted"` includes:
  - `session.cookies_count` (int)
  - `session.cookie_header_length` (int)

---

### 3.8 CSTP connect request

**Endpoint fields:**
- `cstp.path` (string) = `"/CSCOT/"`
- `cstp.url` (string) = `{server.origin}{cstp.path}`

**HTTP request (over TLS):**
- `cstp.request.method` = `"CONNECT"`
- `cstp.request.path` = `cstp.path`
- `cstp.request.headers.host` (string) = `{server.host}` plus `":{server.port}"` when port is not 443
- `cstp.request.headers.user_agent` (string)
- `cstp.request.headers.cookie` (string) = `session.cookie_header`
- `cstp.request.headers.x_cstp_version` (string) = `"1"`
- `cstp.request.headers.x_cstp_hostname` (string) = `client.hostname`
- `cstp.request.headers.x_cstp_address_type` (string) = `"IPv4"`

**Events:**
- `event.type = "cstp.connect.start"` includes `cstp.url`, `cstp.request.headers.x_cstp_version`.

---

### 3.9 CSTP response headers (tunnel configuration)

After a successful CSTP CONNECT, parse HTTP response headers and derive tunnel parameters.

**HTTP response fields:**
- `cstp.response.status` (int)
- `cstp.response.headers` (object: header-name → array of strings)

**Success (v1):**
- `cstp.response.status` in the 2xx range indicates the data channel begins immediately after the HTTP header terminator.

**Tunnel configuration fields (v1):**
- `tun.ipv4.address` (string)
- `tun.ipv4.netmask` (string)
- `tun.mtu` (int)
- `routes.split_include` (array of strings; CIDR or address/mask pairs as received)
- `routes.bypass` (array of strings; routes that must bypass the tunnel)

**Header-to-field mapping (v1):**
- `tun.ipv4.address` comes from response header `X-CSTP-Address`.
- `tun.ipv4.netmask` comes from response header `X-CSTP-Netmask`.
- `tun.mtu` comes from response header `X-CSTP-MTU`.
- Each `X-CSTP-Split-Include` value appends to `routes.split_include`.
- Each `X-CSTP-Bypass-Route` value appends to `routes.bypass`.

**Events:**
- `event.type = "cstp.connect.ok"` includes `tun.ipv4.address`, `tun.mtu`, `routes.split_include_count`, `routes.bypass_count`.
- `event.type = "cstp.connect.failed"` includes `cstp.response.status` and a short `error.message`.

---

### 3.10 Keepalive / DPD messages

**Fields:**
- `keepalive.interval_ms` (int)
- `dpd.interval_ms` (int)
- `dpd.timeout_ms` (int)

**Rules (v1):**
- If no inbound CSTP payload is observed for `dpd.interval_ms`, send a DPD probe on the CSTP channel.
- If no inbound CSTP payload is observed for `dpd.timeout_ms` after a DPD probe, trigger reconnect (see §3.11).

**Events:**
- `event.type = "cstp.dpd.sent"` includes `dpd.interval_ms`.
- `event.type = "cstp.dpd.timeout"` includes `dpd.timeout_ms`.

**Note:** The precise on-wire representation of DPD and keepalive frames is treated as an unknown to be resolved by measurable probes (see §4).

---

### 3.11 Disconnect and reconnect triggers

**Fields:**
- `reconnect.trigger` (string)
- `reconnect.attempt` (int)
- `reconnect.backoff_ms` (int)

**Reconnect triggers (v1):**
- `"tcp_closed"` — remote closes the TCP connection.
- `"tls_error"` — TLS alerts / handshake failure / verification failure.
- `"http_auth_failed"` — authentication failure per §3.6.
- `"cstp_connect_failed"` — non-2xx CSTP response.
- `"dpd_timeout"` — DPD timeout per §3.10.
- `"route_apply_failed"` — OS tunnel configuration failed.

**Rule (v1):**
- On any trigger above (except user-initiated disconnect), emit `event.type = "reconnect.scheduled"` and re-run the flow from §3.1.

**Events:**
- `event.type = "disconnect"` includes `disconnect.reason`.
- `event.type = "reconnect.scheduled"` includes `reconnect.trigger`, `reconnect.attempt`, `reconnect.backoff_ms`.

---

## 4) Unknowns as measurable probes

Each unknown must be resolved by a reproducible probe producing a concrete artifact.

| Unknown | Probe | Artifact | Blocks |
|---|---|---|---|
| Actual ECNU login entry path (may differ from `/+CSCOE+/logon.html`) | Capture a successful ECNU session trace from client start through successful CSTP connect; identify first HTML login URL requested and any redirects | Redacted PCAP + extracted request/response summary (URL, status, Location) | Login implementation correctness |
| Exact credential form field names (may differ from `username`/`password`) | In the same trace, inspect the POST body keys used during successful login | Redacted PCAP + decoded form keys list | Authentication |
| Session cookie name(s) required for CSTP | Identify which cookies appear after login and which are sent on the CSTP CONNECT request | Redacted PCAP + cookie name set before CONNECT | Authentication → CSTP transition |
| Whether a CSRF/hidden token is required | Compare successful vs intentionally failed attempts; check for hidden input fields and whether missing token changes server behavior | HTML snapshots (redacted) + request diffs | Authentication robustness |
| Minimum CSTP request headers required by ECNU | Iteratively remove optional headers from CONNECT request and observe server response status and headers | Test log + PCAP snippets for each variant | CSTP connect stability |
| On-wire DPD/keepalive frame format | During a stable session, measure bytes sent during idle periods and correlate with server responses | Redacted PCAP + byte-level annotated timeline | Long-lived connection reliability |
| Split-include value format (CIDR vs addr/mask) | Record `X-CSTP-Split-Include` values and validate parsing against OS route installation | Redacted header dump + route-apply logs | Split routing |
| Server bypass route header and semantics | Record bypass-related headers and verify the resulting OS routes prevent tunnel loopback | Redacted header dump + before/after route table snapshots | Connectivity correctness |

---

## 5) Acceptance criteria (Task A1)

- This document contains **no placeholder markers** and is internally consistent.
- The v1 scope explicitly **does not** claim support for arbitrary OpenConnect extra arguments.
- The spec does not introduce any production dependency on OpenConnect implementation.
- The repository validation grep defined in the task instructions produces **no matches** for this file.
