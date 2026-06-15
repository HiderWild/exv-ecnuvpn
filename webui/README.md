# EXV WebUI / Desktop App

Vue 3 + TypeScript frontend for the EXV VPN client. The production desktop
target is migrating to a native **platform WebView shell** that reuses the same
renderer without bundling Electron/Chromium. Electron remains temporarily as a
development and migration adapter.

## Architecture

```
webui/
  src/               Vue 3 SPA (pages, components, stores, API client)
  host/              Neutral desktop host contract shared by all shells
  desktop/           Temporary Electron main process + preload adapter
  dist/              Vite build output (consumed by embed_assets.py)
  dist-electron/     Compiled Electron main/preload (consumed by electron-builder)
  release/           Packaged desktop app artifacts (after desktop:build)
```

The Vue frontend is the same codebase in both modes:

- **Native WebView desktop mode**: The `exv-ui` shell loads the Vite renderer
  from packaged assets and forwards `window.ecnuVpn` requests through the
  neutral host bridge to native core/helper.

- **Electron migration mode**: The Electron main process (`desktop/main/index.ts`) spawns the native `exv` binary and communicates via IPC. The preload script (`desktop/preload/index.ts`) exposes the same `window.ecnuVpn` API. The renderer detects this object and routes API calls through the neutral host client (`src/api/host.ts`).

- **Browser mode**: The C++ binary embeds the Vue build output (`webui/dist/`) into `src/webui_assets.hpp` via `scripts/embed_assets.py`, and serves it through the built-in HTTP server (cpp-httplib). The frontend makes HTTP requests to `/api/*` endpoints on `localhost:18080`.

## Pages

| Page | Description |
|------|-------------|
| AuthPage | Username and password entry |
| DashboardPage | VPN status, connect/disconnect, traffic stats |
| RoutesPage | Split-tunnel route management (add/remove CIDR routes) |
| ServicePage | Privileged helper service install/uninstall/status |
| SettingsPage | VPN server, MTU, WebUI, and other settings |
| LogsPage | Real-time log viewer |

State management uses Pinia stores (`src/stores/`).

## Build Commands

### Frontend only (for embedded WebUI)

```bash
cd webui
npm install
npm run build
```

This produces `dist/` which is consumed by `scripts/embed_assets.py` during the C++ build. The native binary then embeds these assets and serves them at `http://127.0.0.1:18080/`.

### Desktop app (native WebView shell)

```bash
cd webui
pnpm run webview:compile
pnpm run webview:package
```

`webview:package` creates a native package layout under
`build/<platform>/webview/package/ECNU VPN/`:

- `exv-ui` or `exv-ui.exe`
- `bin/exv`
- `bin/exv-helper`
- `webui/index.html`
- `webui/assets/*`

The package script rejects Electron/Chromium payloads such as `electron.exe`,
`Electron Framework.framework`, and `chromium.pak`.

### Desktop app (Electron migration adapter)

```bash
cd webui
npm install
npm run build          # Build the Vue frontend first
npm run build:electron # Compile the Electron main/preload TypeScript
npm run prepare:native # Copy the native exv binary into native/bin/
npm run desktop:build  # Package with electron-builder
```

`desktop:build` runs all four steps above in sequence. Artifacts go to `webui/release/`:

- **Windows**: `ECNU-VPN-<version>-portable.exe` (single-file portable) and `ECNU VPN Setup <version>.exe` (NSIS installer)
- **macOS**: `ECNU VPN-<version>.dmg`

### Development (live reload)

For desktop development (requires the native `exv` binary already built, or set `EXV_PATH`):

```bash
cd webui
npm run desktop:dev    # Vite dev server + Electron with hot reload
```

## Desktop API Routing

The API layer (`src/api/host.ts`) supports desktop host mode through
`window.ecnuVpn`. In Electron migration mode, calls still reach
`ipcRenderer.invoke()` via the preload adapter. In native WebView mode, the same
request envelope is handled by the platform host bridge. Browser `/api/*`
compatibility is intentionally not supported as a replacement for the desktop
host bridge.

## Build Dependency

The **frontend must be built before the C++ native build**. The CMake build runs `scripts/embed_assets.py`, which reads `webui/dist/`. If `dist/` does not exist, the build will fail. See the "Build Order" section in the project root README for the full sequence.
