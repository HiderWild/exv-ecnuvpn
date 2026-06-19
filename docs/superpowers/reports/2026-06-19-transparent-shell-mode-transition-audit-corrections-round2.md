# Transparent Shell Mode Transition Audit Corrections Round 2

Date: 2026-06-19

## Review Source

After commit `ea88d66`, three focused Codex Spark audits were run:

- Renderer transparent shell layout and icon usage.
- Win32 WebView2 host titlebar, tray, and transparency behavior.
- Darwin host and packaging behavior.

Spark findings were reviewed by GPT-5.4 before being accepted.

## Accepted Corrections

### R2-1: Global body background paints transparent resize area

Status: accepted.

`webui/src/style.css` painted `body` with `bg-bg`. During an advanced to minimal transition, the renderer `mode-transition-surface` shrinks while the physical WebView remains larger. The area outside the transition surface therefore fell back to the dark page background instead of true WebView/window transparency.

Correction:

- Make `html`, `body`, and `#app` transparent.
- Keep dark fill on the actual app surfaces rather than the document root.
- Add host static tests that reject `body` using `bg-bg` and require transparent root backgrounds.

### R2-2: macOS manual acceptance missed opacity and icon-mask checks

Status: accepted.

The manual Phase 7 template explicitly checked the Windows transparent shell area, but macOS only checked native traffic lights and direction timing.

Correction:

- Add macOS checks for immediate app icon masking.
- Add macOS checks that transparent shell area is not painted as an opaque panel.
- Add macOS checks that the app icon mask remains visible through transition settle.
- Lock these checklist lines in host static tests.

## Deferred Follow-Up

### R2-3: Automated macOS visual verification

Status: deferred to a macOS execution environment.

GPT-5.4 confirmed that current automated macOS checks are source-string and package-shape checks, not screenshot or pixel-level native-window visual checks. This Windows environment cannot run the macOS native shell. The manual checklist now records the missing visual invariants; a future macOS runner should add screenshot or equivalent native UI smoke coverage for transparent shell opacity and traffic-light placement.

## Rejected Findings

### R2-4: Package script must copy native icon files into the flat package

Status: rejected.

The current runtime paths do not require `icon.ico` or `icon.icns` to be copied into the flat package. Windows embeds the icon resource in the executable, macOS status item icon is drawn in code, and renderer icon usage is packaged through the WebUI dist assets.

## Verification Plan

Run after corrections:

- `pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/webview-package-policy.test.ts`
- `pnpm --dir webui test:host`
- `pnpm --dir webui webview:compile`
- `python scripts\package_ui_shell.py --output-root build\windows\webview\package-audit`
- `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows-packaging-smoke.ps1 -PackageRoot "build\windows\webview\package-audit\ECNU VPN"`
- `git diff --check`

Known environment limitation:

- The default package directory and core IPC pipe can be locked by a currently running `exv-ui.exe`/`exv.exe`; use the audit package root or close the running shell before running default-package overwrite and lifecycle tests.
