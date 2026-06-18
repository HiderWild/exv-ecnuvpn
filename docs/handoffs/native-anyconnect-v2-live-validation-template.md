# Native-Only Live Validation Handoff Template

> Create a dated copy only after a real run, for example
> `docs/handoffs/YYYY-MM-DD-native-only-live-validation.md`.
> Do not commit raw logs, packet captures, cookies, passwords, SAML data,
> challenge responses, or packet payloads.

## Run Metadata

| Field | Value |
|---|---|
| Date | |
| Tester | |
| Platform | Windows / macOS |
| Build commit | |
| Gateway | redacted |
| Helper mode | resident / transient |

## Native-Only Preconditions

- [ ] No OpenConnect process is running.
- [ ] No `__vpn-supervisor` process is running.
- [ ] Runtime status reports `engine=native` and `source=native`.
- [ ] Package evidence contains no OpenConnect binaries, libraries, or scripts.

## Phase Gates

- [ ] P0: XML auth + CSTP CONNECT reaches success or a structured auth/CSD/SAML error.
- [ ] P1: DNS, routes, and liveness work on Windows and/or macOS.
- [ ] P2: challenge/group/CSD/DTLS fallback/reconnect behavior is verified or explicitly marked not exercised.
- [ ] P3: native-only process/package evidence is attached in redacted form.

## Redaction Checklist

- [ ] Password values redacted.
- [ ] `webvpn=` values redacted.
- [ ] `<session-token>` values redacted.
- [ ] Opaque values redacted.
- [ ] SAML values redacted.
- [ ] Challenge responses redacted.
- [ ] Cookies and cookie headers redacted.
- [ ] Packet payloads redacted.

## Evidence Summary

Use summaries and hashes where possible. Do not paste raw secrets.

- Runtime status:
- Process snapshot:
- Package artifact snapshot:
- Auth/CSTP outcome:
- DNS/routes/liveness outcome:
- Challenge/group/CSD/DTLS/reconnect outcome:

## Result

Release live-validation status: PASS / FAIL / NOT LIVE-VALIDATED
