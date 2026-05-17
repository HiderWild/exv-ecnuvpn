# EXV WebUI / Desktop App

Vue 3 + TypeScript frontend for the EXV VPN client, running either inside an **Electron desktop shell** (macOS / Windows) or as a **browser-based WebUI** served by the native C++ binary.

## Architecture

```
webui/
  src/               Vue 3 SPA (pages, components, stores, API client)
  desktop/           Electron main process + preload script
  dist/              Vite build output (consumed by embed_assets.py)
  dist-electron/     Compiled Electron main/preload (consumed by electron-builder)
  release/           Packaged desktop app artifacts (after desktop:build)
```

The Vue frontend is the same codebase in both modes:

- **Desktop mode**: The Electron main process (`desktop/main/index.ts`) spawns the native `exv` binary and communicates via IPC. The preload script (`desktop/preload/index.ts`) exposes a `window.ecnuVpn` API. The renderer detects this object and routes API calls through Electron IPC instead of HTTP (`src/api/desktop.ts`).

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

### Desktop app (Electron)

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

For browser-only development:

```bash
cd webui
npm run dev            # Vite dev server on http://localhost:5173
```

For desktop development (requires the native `exv` binary already built, or set `EXV_PATH`):

```bash
cd webui
npm run desktop:dev    # Vite dev server + Electron with hot reload
```

## Desktop vs Browser API Routing

The API layer (`src/api/desktop.ts`) checks for `window.ecnuVpn`:

- If present (Electron desktop mode): calls go through `ipcRenderer.invoke()` to the Electron main process, which execs `exv desktop-rpc <action>` and returns JSON.
- If absent (browser WebUI mode): calls go through `axios` HTTP requests to the embedded WebUI server's `/api/*` endpoints.

Both paths return the same JSON shapes, so pages and stores are mode-agnostic.

## Build Dependency

The **frontend must be built before the C++ native build**. The CMake build runs `scripts/embed_assets.py`, which reads `webui/dist/`. If `dist/` does not exist, the build will fail. See the "Build Order" section in the project root README for the full sequence.
