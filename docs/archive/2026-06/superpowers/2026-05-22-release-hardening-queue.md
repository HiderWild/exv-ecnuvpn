# Release Hardening Queue

> Status: CLOSED FOR NATIVE REPLACEMENT LANE / HISTORICAL RELEASE HARDENING QUEUE.
> Closure date: 2026-05-31.
> Date: 2026-05-22.
> Branch: `integration/platform-convergence-next`.
> Prerequisite: this queue activates after the `develop` merge gate (R4) in `docs/superpowers/plans/2026-05-22-develop-merge-and-release-readiness.md` passes.
> Native successor: `docs/superpowers/plans/2026-05-31-native-openconnect-replacement-phase3-production-readiness.md`.
> Closure summary: OpenConnect runtime packaging requirements in this historical queue are superseded for the native replacement lane. Keep this queue only for historical release hardening context and legacy fallback diagnostics.

## 1. Purpose

This document tracks release-only work that should NOT block the `develop` merge. Items here are post-merge hardening: packaging, distribution, CI, documentation, and low-priority code debt that matters for a public release but does not affect the functional correctness already proven by the R1/R2 develop gates.

The merge gate stays focused on regressions and functional blockers. Anything that can ship in a follow-up release or that only affects non-primary user paths belongs here.

## 2. Release Hardening Items

| # | Item | Owner | Validation Command | Pass/Fail Criterion | Source Plan |
|---|------|-------|--------------------|----------------------|-------------|
| RH-1 | Windows portable and installer parity | Build/package lane | Build both NSIS installer and portable targets; launch each; complete one full connect/disconnect cycle from packaged UI | Both targets locate native binary and runtime without manual env vars; service install/uninstall works from both; no missing DLL or script | 2026-05-17-windows-desktop-full-ui-closure (W6, W8) |
| RH-2 | Windows driver/runtime readiness workflow | Windows release lane | Remove Wintun/TAP from packaged `resources/bin`; launch UI; observe driver/runtime status and UI guidance | UI shows actionable next step for each missing asset; re-staging and reconnect succeeds; no dead-end error state | 2026-05-17-windows-desktop-full-ui-closure (W5) |
| RH-3 | Windows no-service elevated fallback (R1.4 scenarios) | Windows release lane | Uninstall helper service; launch UI as normal user; attempt one-time elevated connect; accept UAC; disconnect; repeat and cancel UAC | One-time elevated connect/disconnect works or UI clearly directs user to install service; UAC cancellation does not leave UI stuck | 2026-05-22-develop-merge-and-release-readiness (R1.4), 2026-05-21-develop-merge-validation (D1.4) |
| RH-4 | macOS packaged .app and DMG launch verification | macOS release lane | Run `npm run desktop:build`; launch the resulting `.app` from Finder (not dev mode); complete one connect/disconnect cycle | Packaged `.app` locates native binary and runtime without Homebrew paths; no `Killed: 9` from signing/quarantine; DMG mounts and app launches | 2026-05-17-macos-desktop-full-ui-closure (M7, M8), 2026-05-22-develop-merge-and-release-readiness (R2.4) |
| RH-5 | macOS helper-installed and helper-missing packaged scenarios | macOS release lane | From packaged `.app`: install helper, connect, disconnect, uninstall helper; then attempt one-time elevated connect without helper | Both paths work from packaged app (not just dev mode); helper-missing path shows structured cancellation on privilege denial | 2026-05-17-macos-desktop-full-ui-closure (M8), 2026-05-22-develop-merge-and-release-readiness (R2.2, R2.3) |
| RH-6 | GitHub Actions or equivalent reproducible CI | Release engineering | Push a commit to a CI-tracked branch; confirm native build + test + webui build + electron build all run on Windows, macOS, and Linux runners | All three platform builds pass in CI; artifact upload succeeds; no manual build step required for a green run | 2026-05-08-cross-platform-convergence (Phase 3), 2026-05-21-develop-merge-validation (D5.2) |
| RH-7 | README/user guide cleanup for desktop-first workflow | Docs lane | Read `README.md`, `README_CN.md`, `docs/user_guide.md`; verify no passage implies browser WebUI is the primary entry point | All three docs present desktop app as primary; CLI and browser WebUI described as compatibility/debug paths; no conflicting instructions | 2026-05-17-windows-desktop-full-ui-closure (W4), 2026-05-17-macos-desktop-full-ui-closure (M5) |
| RH-8 | Historical document encoding cleanup | Docs lane | Open each file under `docs/merge-playbooks/` and closed plan files; check for mojibake or non-UTF-8 artifacts | All audit and closure documents render cleanly in UTF-8; no garbled CJK or replacement characters in final published docs | 2026-05-21-develop-merge-validation (D3.3), 2026-05-22-develop-merge-and-release-readiness (R3.3) |
| RH-9 | Move small runtime_status.cpp platform #ifdef branches behind platform-specific helpers | Native repair lane | `grep -n '#ifdef\|#if defined' src/platform/common/runtime_status.cpp`; confirm no platform-conditional logic remains in the common file | Common `runtime_status.cpp` has zero `#ifdef`/`#if defined` guards; platform branches live in `src/platform/{darwin,win32,linux}/` helpers | 2026-05-21-develop-merge-validation (D3.3), 2026-05-22-develop-merge-and-release-readiness (R3.3) |
| RH-10 | Replace renderer fallback string matching for elevation_denied with structured fallback path | Desktop repair lane | `grep -rn 'elevation_denied\|elevation-denied' webui/src/`; confirm no string-comparison fallback remains | Renderer uses only `error_type` enum for elevation-denied detection; no `message.includes('elevation_denied')` or equivalent string check | 2026-05-21-develop-merge-validation (D3.3), 2026-05-22-develop-merge-and-release-readiness (R3.3) |
| RH-11 | Clean mojibake/encoding artifacts in historical audit and closure documents | Docs lane | Same validation as RH-8 but scoped to historical closure documents specifically | Historical closure docs render cleanly; no encoding artifacts remain in any file intended for GitHub publication | 2026-05-22-develop-merge-and-release-readiness (R3.3) |
| RH-12 | Revisit Linux-specific items from 2026-05-08 convergence plan | Linux release lane | Build and test on Linux (Docker or native); verify openconnect install logic, systemd helper, and AF_UNIX IPC | Linux native build passes; helper daemon compiles and runs under systemd; openconnect install uses correct package manager detection | 2026-05-08-cross-platform-convergence (Phase 1, Phase 2), 2026-05-22-develop-merge-and-release-readiness (R3.3) |

## 3. Dependency Notes

| Item | Depends on develop merge? | Other dependencies | Notes |
|------|--------------------------|--------------------|-------|
| RH-1 | Yes | RH-2 for runtime/driver completeness | Parity testing requires the final merged codebase on `develop` |
| RH-2 | Yes | None | Driver readiness workflow may need new UI fields; must not change shared contract without revalidation |
| RH-3 | No (can draft during R1) | None | R1.4 scenarios are release-blockers; this item captures the full fallback matrix beyond the develop-gate subset |
| RH-4 | Yes | RH-5 for full packaged verification | DMG signing/notarization may require certificates not yet available; document as a gate if absent |
| RH-5 | Yes | RH-4 for packaged app baseline | Must be tested from packaged `.app`, not `desktop:dev` |
| RH-6 | No (can draft during validation) | None | CI setup can begin before merge; final workflow validation requires the merged `develop` branch |
| RH-7 | No (can draft during validation) | None | Doc cleanup is safe to start anytime; final wording should reflect the merged state |
| RH-8 | No | RH-11 overlaps scope | RH-8 covers all docs; RH-11 is a subset for historical closure docs |
| RH-9 | Yes | None | Code change; must be done on a branch from `develop` after merge |
| RH-10 | Yes | None | Code change; must be done on a branch from `develop` after merge |
| RH-11 | No | RH-8 overlaps scope | Subset of RH-8; can be done independently if RH-8 is split |
| RH-12 | Yes | RH-6 for CI validation | Linux work was deferred from the original convergence plan; revisit only after Windows/macOS are stable on `develop` |

## 4. Done Definition

Each item in this queue is done when all of the following hold:

1. **Owner** is assigned and has produced the work.
2. **Validation command** has been run and the result recorded (pass or fail with evidence).
3. **Pass/fail criterion** is met; if a criterion cannot be met (e.g., missing certificate for notarization), the item is recorded as "blocked" with the specific blocker and a follow-up action.
4. **No develop regression**: the item's changes do not break any previously passed R1/R2 develop gate scenario.
5. **Commit discipline**: code changes follow the `repair:` or `hardening:` prefix convention; doc-only changes have no prefix requirement.

Items that are blocked by external factors (missing certificates, unavailable CI runners, no Linux test environment) are not considered done. They remain open with the blocker documented until the external factor is resolved or the item is explicitly descoped from the current release.
