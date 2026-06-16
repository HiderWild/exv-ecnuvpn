# EXV WebUI

Vue 3 + TypeScript renderer for the EXV desktop shell and browser compatibility UI.

The production desktop target is the native WebView shell (`exv-ui`). It loads this renderer from packaged assets and communicates with the C++ core through the neutral host contract under `webui/host`.

## Layout

```text
webui/
  src/               Vue SPA pages, components, stores, and API client
  host/              Neutral desktop host contract and host tests
  desktop/shared/    Generated public desktop contract constants
  scripts/           WebView package and host test helpers
```

## Build

Renderer only:

```bash
pnpm run webview:compile
```

Native WebView package:

```bash
pnpm run webview:package
```

The platform scripts build the renderer, native shell, core, helper, and package together:

```bash
../scripts/build-windows.ps1 desktop
../scripts/build-macos.sh desktop
../scripts/build-linux.sh desktop
```

Package output:

```text
build/<platform>/webview/package/ECNU VPN
```

Package contents include `exv-ui`, `exv-ui.args`, `bin/exv`, `bin/exv-helper`,
and `webui/index.html`. `exv-ui.args` records the packaged `--exv` and
`--renderer-index` targets so launchers and smoke tests validate the same
contract.

## Tests

Host and package policy tests:

```bash
pnpm run test:host
```

Contract-only test:

```bash
pnpm run test:contract
```

The native WebView package script rejects bundled Electron or Chromium payloads such as `electron.exe`, `Electron Framework.framework`, and `chromium.pak`.

## Runtime Contract

Renderer code talks to `window.ecnuVpn` through `src/api/host.ts`. The native WebView host implements that object in platform code and forwards requests to core using the generated desktop RPC contract.

Browser `/api/*` compatibility remains for diagnostics and legacy use, but it is not the production desktop bridge.
