# Cross-Platform WebView Shell Design

> Implementation status: Windows native WebView packaging has been verified in
> `docs/superpowers/reports/2026-06-16-webview-shell-acceptance-report.md`.
> macOS acceptance must be run on SSH host `macmini` from
> `/Users/tomli/Development/Projects/CPP/ECNU-VPN`. Linux acceptance still
> requires a Linux host. The migration is not globally accepted while the
> macOS/Linux host implementations still return stub exit code `70`.

## Summary

The desktop UI should move away from Electron as the default packaged shell on
all supported platforms. The renderer remains the same Vue SPA, and the native
core remains the authority for VPN, config, helper, service, routes, logs, and
runtime behavior. The shell becomes a thin platform-specific WebView host:

- Windows: WebView2 with the Evergreen Runtime.
- macOS: WKWebView.
- Linux: WebKitGTK.

The migration is phased. Electron production packaging has been retired on the
current branch; the final release shape does not package a full Chromium copy.
The remaining parity work is macOS WKWebView and Linux WebKitGTK host
implementation plus host-specific acceptance.

## Current State

The repository has a clean renderer/backend split at the business boundary and
now uses a neutral desktop host contract:

- `webui/src` is a Vue 3 SPA.
- `webui/src/api/desktop.ts` routes renderer requests through
  `window.ecnuVpn`.
- `webui/host/shared/host-contract.ts` owns the TypeScript host contract.
- `webui/host/shared/generated/system-contract.ts` is generated from the
  repository contract manifest.
- `src/app/ui_shell` owns native shell orchestration, core process transport,
  renderer asset resolution, and the platform-neutral `UiWindow` runtime.
- `src/platform/win32/ui_shell` owns the WebView2 host, runtime detection, and
  Evergreen bootstrapper policy.
- `src/platform/darwin/ui_shell` and `src/platform/linux/ui_shell` expose
  `UiWindow` factories, but their `run(...)` implementations still return the
  migration stub exit code `70`.
- `scripts/package_ui_shell.py` packages the native WebView shell and native
  binaries; production packaging no longer uses Electron or `electron-builder`.

The remaining weak boundary is not renderer coupling, but platform parity:
macOS and Linux still need real WebView hosts and host-specific acceptance logs.

## Goals

- Keep the Vue renderer and generated RPC contract reusable across all desktop
  shells.
- Make WebView the default packaging strategy on Windows, macOS, and Linux.
- Avoid bundling a full browser engine where the OS provides a maintained
  WebView runtime.
- Preserve the existing core process model and desktop RPC envelope.
- Keep privileged helper behavior and service routing inside native core/helper,
  not the UI host.
- Keep Electron only as a temporary fallback during migration and as an
  optional developer shell until it is no longer needed.
- Make missing runtime behavior explicit:
  - Windows can install WebView2 Evergreen Runtime if missing.
  - macOS should fail only on unsupported OS versions where WKWebView is not
    viable.
  - Linux should detect WebKitGTK packages and report actionable installation
    guidance rather than silently failing.

## Non-Goals

- Rewriting the Vue renderer.
- Changing helper/core privileged execution semantics.
- Moving platform-native UI widgets into the renderer.
- Embedding fixed WebView runtimes as the default package shape.
- Supporting direct browser `/api/*` mode as a replacement for the desktop
  host bridge in this migration.

## Packaging Approaches Considered

### Option A: Electron As Default, WebView As Optional

This is the lowest risk in the short term, but it does not satisfy the target:
the default package still ships Chromium and keeps Electron as the architectural
center.

### Option B: Windows WebView2 First, Other Platforms Later

This reduces immediate complexity and maps well to WebView2 Evergreen, but it
creates a long-lived split where Windows and non-Windows desktop shells have
different architecture and tests.

### Option C: Neutral Host Contract First, Then Platform WebViews

This is the chosen approach. First extract a neutral desktop host contract from
the Electron bridge. Then implement Windows, macOS, and Linux WebView hosts
behind the same contract in phases. Electron becomes an adapter that uses the
same contract while it remains in the tree.

## Adopted Architecture

```text
webui/src
  Vue renderer
  neutral desktop host API client

webui/host/shared
  generated system contract
  host-neutral bridge types

src/app/ui_shell
  shared native shell process orchestration
  renderer asset loading
  core process lifecycle
  desktop RPC bridge
  modal/tray/window abstractions

src/platform/win32/ui_shell
  WebView2 host
  Evergreen Runtime detection/install bootstrap
  Windows tray/window/modal implementation

src/platform/darwin/ui_shell
  WKWebView host
  macOS menu/tray/window/modal implementation

src/platform/linux/ui_shell
  WebKitGTK host
  Linux tray/window/modal implementation

webui/desktop/electron
  temporary Electron adapter for migration/dev fallback
```

Dependency direction:

```text
renderer -> neutral host bridge contract
ui_shell common -> core RPC client / app lifecycle abstractions
platform ui_shell -> ui_shell common + OS WebView APIs
Electron adapter -> neutral host bridge contract, temporary only
```

The renderer sees one capability: `window.ecnuVpn` or its renamed neutral
successor. The implementation behind that object is allowed to be Electron,
WebView2, WKWebView, or WebKitGTK, but the renderer must not branch on those
shell engines for business operations.

## Frontend And Backend Coupling Rules

The renderer may depend on:

- generated desktop RPC action names,
- neutral host bridge TypeScript types,
- UI-only host services such as window mode, modal prompts, close handling, and
  event subscription.

The renderer must not depend on:

- Electron imports,
- WebView2/WKWebView/WebKitGTK APIs,
- native binary paths,
- platform service managers,
- helper IPC implementation details,
- shell-specific IPC channel names.

The native shell may depend on:

- platform WebView runtime APIs,
- native process launch and lifecycle APIs,
- desktop RPC bridge,
- generated system contract.

The native shell must not implement business authority. It forwards user intent
to core/helper and renders results.

## WebView Runtime Strategy

### Windows

Use Microsoft Edge WebView2 Evergreen Runtime. Production must use the WebView2
Runtime as the backing platform, not Microsoft Edge Stable as a browser
dependency.

Runtime handling:

1. Detect WebView2 Evergreen Runtime before creating the main WebView.
2. If present, launch normally.
3. If missing and network is available, download and run the Evergreen
   Bootstrapper from the Microsoft-supported distribution endpoint.
4. If bootstrap install fails or policy blocks it, show a native error with a
   link/instructions for manual Evergreen Runtime installation.
5. Do not default to Fixed Version runtime because it increases package size and
   shifts browser engine update responsibility into this project.

### macOS

Use WKWebView. It is provided by the OS WebKit framework. The app should define
a minimum supported macOS version where WKWebView behavior is acceptable and
fail early on older systems.

### Linux

Use WebKitGTK. Package metadata must declare the required shared library/runtime
dependency for each Linux package format. At runtime the launcher should detect
missing WebKitGTK and show actionable distro-specific guidance where practical.

## Desktop Host Contract

The existing `window.ecnuVpn` API should become a neutral host bridge. The final
name can stay `window.ecnuVpn` for renderer compatibility, but source ownership
should move out of Electron-specific folders.

Required bridge areas:

- `status.get`
- `vpn.connect`
- `vpn.disconnect`
- `config.getAuth/saveAuth/getSettings/saveSettings/getKey`
- `routes.list/add/remove/reset`
- `service.status/install/uninstall`
- `runtime.status`
- `drivers.status/install`
- `logs.list`
- event subscription
- modal prompt requests and responses
- close handling
- core restart/quit
- window mode

The bridge transport is shell-specific:

- Electron: `ipcRenderer.invoke` during migration.
- Windows WebView2: host object or web message bridge.
- macOS WKWebView: script message handler bridge.
- Linux WebKitGTK: JavaScriptCore/WebKit user content manager bridge.

All transports must preserve the same request/response envelope and generated
action allowlist.

## Renderer Asset Strategy

The renderer remains a static Vite build. WebView hosts load either:

- dev server URL in development, or
- packaged `index.html` and static assets in production.

The current embedded C++ WebUI asset path is not the desktop shell target. It
may remain for CLI/browser diagnostics, but the desktop package should use the
same renderer build output as a local app asset.

## Security Model

- Disable Node-like renderer access in all shells.
- Only expose the minimal host bridge object.
- Validate every incoming action against the generated contract allowlist.
- Keep credentials out of helper messages and out of shell logs.
- Treat renderer messages as untrusted input even when loaded from local assets.
- Use OS-specific origin/source checks where the WebView bridge supports them.
- Keep WebView2 runtime installation behind explicit native UX and clear error
  reporting; do not silently elevate or install browser components.

## Phased Migration

### Phase 1: Neutral Host Contract

- Rename Electron-specific bridge ownership to neutral desktop host ownership.
- Move shared TypeScript contract out of Electron-named paths.
- Add architecture tests that block renderer imports from Electron and shell
  engine APIs.
- Keep Electron adapter passing existing tests.

### Phase 2: Common Native UI Shell

- Introduce native C++ shell abstractions for:
  - core process lifecycle,
  - desktop RPC bridge,
  - event pump,
  - window lifecycle,
  - modal prompts,
  - tray/menu actions,
  - renderer asset resolution.
- Do not change core/helper contracts.

### Phase 3: Windows WebView2 Host

- Add WebView2 host executable path.
- Detect Evergreen Runtime.
- Add online Evergreen Bootstrapper install flow when runtime is missing.
- Package without Electron/Chromium.
- Reach feature parity with current Electron Windows behavior.

### Phase 4: macOS WKWebView Host

- Add WKWebView host.
- Recreate window, tray/menu, modal, event bridge, and core process handling.
- Package as a native macOS app without Electron.

### Phase 5: Linux WebKitGTK Host

- Add WebKitGTK host.
- Define package/runtime dependencies.
- Recreate window, tray/menu where supported, modal, event bridge, and core
  process handling.
- Package without Electron.

### Phase 6: Electron Retirement

- Remove Electron from default build and packaging scripts.
- Keep a temporary dev-only adapter only if it has clear value and no production
  packaging path.
- Remove `electron-builder` package output after all platform WebView packages
  pass acceptance.

## Testing Strategy

Contract tests:

- Generated RPC action allowlist matches renderer host bridge.
- Shell bridge rejects unknown actions on every platform.
- Renderer has no direct Electron/WebView platform imports.

Host integration tests:

- Core starts, reports ready, handles restart and quit.
- `status.get`, connect/disconnect, config, routes, service, runtime, drivers,
  logs, and events work through the host bridge.
- Modal prompts round-trip correctly.
- Close-to-tray/quit behavior remains correct.

Runtime tests:

- Windows detects installed WebView2 Evergreen Runtime.
- Windows missing-runtime path attempts Bootstrapper flow only with explicit
  user-facing progress/error behavior.
- macOS rejects unsupported OS versions cleanly.
- Linux reports missing WebKitGTK dependencies cleanly.

Packaging tests:

- WebView packages do not include Electron or Chromium payloads.
- Native `exv` and `exv-helper` are staged correctly.
- Platform package metadata includes required WebView runtime dependencies.

Regression tests:

- Existing desktop RPC tests remain valid through the neutral contract.
- Existing helper/service routing tests continue to pass.
- Existing WebUI build remains valid.

## References

- Microsoft WebView2 distribution guidance:
  <https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution>
- Microsoft Evergreen versus Fixed Version runtime guidance:
  <https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/evergreen-vs-fixed-version>

## Success Criteria

- Windows, macOS, and Linux default packages use platform WebView hosts instead
  of Electron.
- The default app package does not bundle full Chromium.
- The renderer remains shell-neutral and communicates only through the generated
  host bridge contract.
- WebView runtime absence is detected and reported explicitly on every platform.
- Windows can install WebView2 Evergreen Runtime through a controlled online
  bootstrap path when it is missing.
- Electron is not required for production packaging.
