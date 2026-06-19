# Transparent Shell Mode Transition Audit Corrections - Round 3

Date: 2026-06-19

## Review Source

- Final broad Codex Spark audit after round 2 fixes.
- GPT-5.4 adjudication of Spark findings.
- User clarification on resize-transition transparency: during the transition mask, the area outside the mask but inside the physical window must be fully transparent, not dark.

## Accepted Findings

### R3-1 Minimal titlebar identity layout regression

Status: Confirmed defect.

The minimal-mode self-managed titlebar still rendered the icon and `EXV for ECNU` in a single horizontal line. The corrected requirement is to restore the minimal titlebar's vertical identity layout while keeping advanced mode title-free, full-window-width, and floating.

Correction:

- Render minimal identity with an accessible `EXV for ECNU` label.
- Display `EXV` and `for ECNU` as two title lines next to the restored packaged icon.
- Keep advanced mode hiding the titlebar identity.

### R3-2 macOS packaging smoke did not re-verify launch arguments

Status: Confirmed coverage gap.

The package builder validates `exv-ui.args` targets during package creation, but `scripts/macos-packaging-smoke.sh` did not independently re-run the launch-target verifier against an existing macOS WebView package.

Correction:

- Add `package_ui_shell.py --verify-launch-targets-only --package-dir "$PACKAGE_ROOT"` to the macOS packaging smoke package-policy section.
- Keep the existing Electron/Chromium payload rejection.

### R3-3 Resize-transition transparent area acceptance gap

Status: Confirmed acceptance clarification.

During the transition mask, the masked surface can be opaque or blurred, but the area outside that mask and still inside the physical window must remain fully transparent.

Correction:

- Strengthen renderer policy tests to assert that frame, transparent host, and transition surface backgrounds remain transparent.
- Assert that the transition overlay is an absolute layer inside the transition surface, not a fixed full-window overlay.
- Add explicit Windows and macOS manual Phase 7 checklist items for the mask-outside transparent area.

## Verification Plan

- `pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts`
- `pnpm --dir webui test:host`
- `pnpm --dir webui webview:compile`
- `python scripts/package_ui_shell.py --output-root build/windows/webview/package-audit`
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts/windows-packaging-smoke.ps1 -PackageRoot "build/windows/webview/package-audit/ECNU VPN"`
- `git diff --check`
