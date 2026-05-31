# Native Production Release Readiness — Decision (2026-05-31)

**Task:** H2 — Final Native Production Release Decision
**Plan:** `docs/superpowers/plans/2026-05-31-native-openconnect-extraction-completion-plan.md`
**Hosts:** Windows 10.0.26200 x64 + macmini (macOS 26.3.1 arm64, remote). No ECNU
network reachability/credentials and no code-signing identity available on either host.

> **Update 2026-05-31 (revised):** A real macOS host (macmini) became available and the
> macOS **automated** gates have since PASSED — **B4v** (Secure Transport TLS validation)
> and **G1** (macOS build/test/package + forbidden-asset scan). See
> `docs/validation/native-macos-automated-2026-05-31.md`. The only remaining blockers are the
> **live connect** acceptances (F2 on Windows, G2 on macOS) and a code-signing identity —
> all environment-gated, none requiring product changes (one latest-SDK build fix landed:
> `#include <sys/sys_domain.h>` in `src/platform/darwin/native_utun.cpp`).

## Decision

**BLOCKED** — pending environment-gated **live-connect** acceptance (F2/G2) and a
code-signing identity. Every automated gate on both Windows and macOS is now complete
and green. The remaining gates require live ECNU connectivity with credentials and
administrative/sudo privilege.

This is an explicit environment block, not a product failure: no failing test, no
clean-room blocker, and no packaging-composition blocker was found on either platform.

## Evidence Ledger

| Item | Status | Evidence |
| --- | --- | --- |
| P1 CSTP wire framing | PASS (mock; live capture ENV-BLOCKED) | `docs/architecture/cstp-wire-verification-2026-05-31.md` |
| P2 Forwarding data plane | PASS (mock; live concurrent-TLS ENV-BLOCKED) | `docs/architecture/dataplane-forwarding-2026-05-31.md` |
| P3 DPD / keepalive / reconnect | PASS (mock; live DPD timing ENV-BLOCKED) | `docs/architecture/dataplane-forwarding-2026-05-31.md` (P3 section) |
| P4 MTU / routes / split | PASS | `native_engine_contract_test` (full suite 25/25) |
| C2 Packaging composition | PASS | `docs/validation/native-packaging-composition-2026-05-31.md` |
| D1 Durable native session | PASS (store/contract; live supervisor tunnel ENV-BLOCKED) | `native_helper_session_test`; plan §D1 |
| D2 Helper/desktop RPC contract | PASS (contract; live native-failure surfacing ENV-BLOCKED) | `native_helper_session_test` (`test_native_error_codes_map_to_contract_codes`); plan §D2 |
| F1 Windows automated gate | PASS | `docs/validation/native-windows-automated-2026-05-31.md` |
| H1 Clean-room & secret audit | PASS | `docs/security/native-openconnect-replacement-review-2026-05-31.md` |
| **B4v macOS TLS validation** | **PASS** | `docs/validation/native-macos-automated-2026-05-31.md` (macmini, 25/25 incl. `darwin_native_tls_stream_test`) |
| **F2 Windows live connect** | **ENV-BLOCKED** | needs Administrator + ECNU reachability + credentials |
| **G1 macOS automated gate** | **PASS** | `docs/validation/native-macos-automated-2026-05-31.md` (build 100%, 25/25, clean package, no forbidden assets) |
| **G2 macOS live connect** | **ENV-BLOCKED** | needs ECNU reachability + credentials + sudo on macmini |

Automated regression at decision time: `ctest --preset windows-release` =
**25/25 passed**; `npm run build` + `npm run build:electron` +
`npm run desktop:build` exit 0; package forbidden-asset scan empty.

## Review Gate Status

| Gate | Status |
| --- | --- |
| G-A Protocol Freeze (P1–P3) | GREEN (mock-verified; live capture ENV-BLOCKED) |
| G-B Platform & MTU (P4, B4v) | GREEN — P4 GREEN; **B4v PASS on macmini** |
| G-C Durable Session (D1, D2) | GREEN |
| G-D Package Candidate (C2, H1) | GREEN |
| G-E Release (F1, F2, G1, G2, H1, H2) | **BLOCKED** — F1+G1+H1 green; **F2/G2 live-connect ENV-BLOCKED** |

## Exact External Blockers

1. ~~No macOS host~~ — RESOLVED: macmini available; B4v + G1 now PASS.
2. **No ECNU network reachability + credentials** — blocks F2 and G2 live
   connect/disconnect acceptance, and the live-traffic confirmation portions of
   P1/P2/P3/D1/D2.
3. **No administrative session on this host** — blocks live Windows tunnel bring-up
   (Wintun adapter creation requires elevation), already observed when a stale
   elevated `exv-helper.exe` could not be terminated without admin rights.
4. **No code-signing identity** — packages were produced with a placeholder
   signtool path; a release build needs a real signing certificate.

## Recommendation

Promote to **release-candidate (Windows + macOS automated + clean-room verified)**.
Both platforms now pass their full automated build/test/package/clean-room gates.
Final production `pass` requires only:

- an Administrator Windows session with ECNU reachability + credentials (execute F2), and
- ECNU reachability + credentials + sudo on macmini (execute G2 live connect), and
- a signed-build pipeline (code-signing identity).

No further source changes are required to lift the block — only the unavailable
live-network environments and a signing identity. Reopen H2 once that evidence is attached.
