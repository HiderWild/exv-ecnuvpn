# Transparent Shell Mode Transition Audit Corrections

Date: 2026-06-19

## Review Source

After completing the implementation plan, four focused Codex Spark audits were run:

- Renderer transition state and shared frame.
- Windows WebView2 host.
- macOS WKWebView host.
- Packaging, tests, and manual acceptance.

The Spark findings were reviewed by GPT-5.4 before any fix was accepted.

## Accepted Corrections

### C1: Win32 control-zone hit testing bypasses renderer controls

Status: accepted.

The Win32 hit-test implementation returned `HTMINBUTTON`/`HTCLOSE` for the renderer titlebar control zone. That routes clicks through native non-client behavior instead of the renderer buttons, bypassing `AppWindowFrame` transition guards.

Correction:

- Return `HTCLIENT` for the renderer control zone.
- Keep `HTCAPTION` for draggable titlebar space.
- Update the Win32 source guard to reject native caption-button hits.

### B3: Manual acceptance checklist lacks exact transition counts

Status: accepted.

The manual Phase 7 report template contained general transparent-shell checks, but did not mirror the plan's explicit Windows visual acceptance steps: 10 moderate advanced-to-minimal toggles, 10 rapid advanced-to-minimal toggles, 10 rapid minimal-to-advanced toggles, and idle minimize/close checks.

Correction:

- Add the exact counted checks to `manual-observation.md`.
- Extend host static tests so the checklist stays present.

### D2: `resolveClosePrompt` bridge response shape drifts from type contract

Status: accepted.

The renderer type contract declares `resolveClosePrompt(...): Promise<{ ok: true }>`, but both Win32 and Darwin hosts returned an empty data object.

Correction:

- Return `data.ok = true` from both native hosts.
- Add static host contract checks for the response shape.

## Rejected Or Deferred Findings

### B1: Release builds disable new source guards

Status: rejected as a transparent-shell finding.

The new transparent-shell source guards use explicit `if (...) return 1` checks, so they still run under `NDEBUG`. Existing legacy `assert` usage is outside this feature's correction scope.

### B2: Source-string tests are inherently fragile

Status: rejected for this plan.

The implementation plan intentionally uses static host/source contract tests so Windows, macOS, and renderer bridge contracts are enforceable from this environment.

### C2: Win32 raw `window.setMode` compatibility branch sequencing

Status: deferred pending evidence.

The injected renderer bridge aliases `setMode` to `window.resizeForMode`, and no current feature path calls the raw `window.setMode` host action directly.

### D1: Darwin missing or non-positive request bypass

Status: deferred pending evidence.

The implemented renderer always sends a positive monotonically increasing request id. Hardening malformed host calls can be considered separately if a non-renderer caller is introduced.

## Verification Plan

Run after corrections:

- `pnpm --dir webui test:host`
- `cmake --build build-windows\cpp --target win32_webview2_runtime_test exv-ui --config Release`
- `.\build-windows\cpp\win32_webview2_runtime_test.exe`
- `pnpm --dir webui webview:compile`
- `git diff --check`

Then rerun focused Spark audits on the corrected Win32 host, Darwin response shape, and acceptance checklist.

## Follow-Up Audit Corrections

After commit `403e35c`, three narrower Spark audits were run and reviewed by
GPT-5.4 before accepting more changes.

### B4: Operator guidance still used the old 8+ click wording

Status: accepted.

The manual checklist had the corrected 10-count direction-specific checks, but
the script's operator guidance still asked for "8+ clicks".

Correction:

- Update the operator guidance to match the three 10-count toggle checks.
- Add a host static test assertion for the operator guidance text.

### B5: Checklist test did not cover two existing transparent-shell checks

Status: accepted.

The checklist already included visual shrink and transparent-shell paint checks,
but the host static test did not lock those exact items.

Correction:

- Add host static test assertions for both checklist lines.

## Follow-Up Rejected Or Deferred Findings

### A1: Direct behavioral Win32 `resolveClosePrompt` response assertion

Status: rejected for this plan.

The plan's current verification model intentionally uses static host contract
tests for bridge shape, and the direct Win32/Darwin handlers are already locked
to `data.ok = true`.

### D3: Generic async host bridge `resolveClosePrompt` empty response

Status: deferred.

The generic async host bridge still returns an empty data object for local window
actions, but Win32 and Darwin transparent-shell hosts intercept the normal
`resolveClosePrompt` path directly. This is broader bridge-contract cleanup, not
a current transparent-shell regression.
