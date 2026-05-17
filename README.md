# EXV — Smart VPN Client for ECNU

> Split-tunneling VPN client that routes only campus traffic through the tunnel — everything else stays on your local network. Available on **macOS**, **Windows**, and **Linux**.

## Background

When Cisco AnyConnect connects to the campus VPN, **all traffic is routed through the VPN tunnel by default**. This means:

- Accessing domestic services (Baidu, Bilibili, WeChat) is slow because packets detour through the campus VPN
- International traffic also suffers from the extra hop
- Limited campus bandwidth is shared across all traffic

**EXV's solution**: **Split tunneling** — only campus IP ranges are routed through the VPN tunnel; all other traffic uses your default route. After connecting, everyday browsing speed is unaffected while campus resources remain accessible.

## Features

- **Split tunneling** — Only campus traffic goes through VPN; everything else uses your local network
- **Desktop app** — Electron-based GUI for macOS and Windows; manage VPN, config, and logs without a browser
- **Service installation** — One-time elevation installs a privileged helper (launchd on macOS, Windows service on Windows); daily use needs no admin/sudo
- **Encrypted credentials** — AES-256-CBC encrypted password storage with key file permission 0600
- **WebUI** — Browser-based management interface for status, config, and logs (compatibility option; the desktop app is the recommended interface)
- **Auto-reconnect** — Automatic reconnection after disconnection
- **Custom routes** — Add or remove split-tunnel routes at any time
- **VPN server route protection** — Automatically prevents VPN server traffic from being swallowed by the tunnel

## Quick Start

The recommended way to use EXV is through the **desktop app** (macOS / Windows). It bundles the native VPN binary and provides a graphical interface for connecting, configuring routes, and viewing logs — no terminal or browser needed.

For CLI-only or Linux usage, see [Platform Support](#platform-support) below.

### Desktop App (Recommended)

1. Download the latest release for your platform from the Releases page.
2. On **macOS**: Drag to Applications, then run. On first launch the app will prompt for your admin password to install the privileged helper.
3. On **Windows**: Run the NSIS installer (or the portable `.exe`). On first launch, accept the UAC prompt to install the `exv-helper` Windows service.
4. Enter your campus username and password in the desktop app and click Connect.

### Service Installation (All Platforms)

Whether you use the desktop app or the CLI, installing the privileged helper is a one-time step that eliminates the need for repeated `sudo`/admin elevation:

```bash
# macOS / Linux
sudo exv service install

# Windows (run as Administrator)
exv.exe service install
```

After installation, `exv` and `exv stop` work without elevated privileges.

### CLI Quick Start

```bash
# 1. Set username
exv config set username
# > Enter value for username: 20XXXXXXXXX

# 2. Set password (hidden input, encrypted storage)
exv config set password
# >   New password: ••••••••
# >   Confirm password: ••••••••

# 3. Start VPN (no sudo needed after helper is installed)
exv

# 4. Stop VPN
exv stop
```

On first run, the config file and encryption key are created automatically: `~/.ecnuvpn/` on macOS/Linux, `%APPDATA%\ecnuvpn\` on Windows.

## Build Order

The project has a strict build dependency chain that must be followed:

1. **Frontend build** — `cd webui && npm install && npm run build`
2. **Native build** — `cmake -B build && cmake --build build`
3. **Desktop build** (optional) — `cd webui && npm run desktop:build`

The frontend must be built first because the C++ build runs `scripts/embed_assets.py`, which reads `webui/dist/` and generates `src/webui_assets.hpp`. If `webui/dist/` does not exist, the native build will fail.

## Building from Source

### Prerequisites

- **openconnect** — VPN connection core
- **CMake** — Build system
- **Node.js** (v18+) — Required to build the frontend (which must be built before the native binary)
- **OpenSSL** (Linux only) — AES-256-CBC on Linux; macOS uses CommonCrypto, Windows uses CNG/BCrypt

### Full Build

```bash
git clone <repo>
cd ECNU-VPN

# 1. Build the frontend first
cd webui
npm install
npm run build
cd ..

# 2. Build the native binary
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build

# 3. Install to system path
sudo cmake --install build    # macOS / Linux
```

### Installing the Privileged Helper (One-Time)

```bash
sudo exv service install
```

This copies `exv` to `/usr/local/bin/exv` (macOS/Linux) or registers the Windows service, and sets up the privileged helper daemon. **After installation, daily use does not require sudo/admin.**

## Platform Support

### macOS

```bash
brew install openconnect
cd webui && npm install && npm run build && cd ..
cmake -B build && cmake --build build
sudo exv service install
```

### Linux (Ubuntu/Debian)

```bash
sudo apt install openconnect libssl-dev cmake build-essential
cd webui && npm install && npm run build && cd ..
cmake -B build && cmake --build build
sudo ./scripts/install-linux.sh
```

### Linux (Fedora/RHEL)

```bash
sudo dnf install openconnect openssl-devel cmake gcc-c++
cd webui && npm install && npm run build && cd ..
cmake -B build && cmake --build build
sudo ./scripts/install-linux.sh
```

### Windows

1. Install [openconnect-gui](https://github.com/openconnect/openconnect-gui/releases) (provides `openconnect.exe` + GnuTLS runtime DLLs).
2. Build the frontend first: `cd webui && npm install && npm run build && cd ..`
3. Build: `cmake -B build && cmake --build build --config Release`
   - The desktop client uses Windows **BCrypt** (CNG) for AES-256-CBC, so OpenSSL is no longer required on Windows.
4. Run as Administrator: `.\build\Release\exv.exe service install`

### Windows Desktop Packaging (Portable + Installer)

The Electron-based desktop UI can be packaged for Windows in two flavours:

```powershell
# 1. Stage the openconnect runtime once (DLLs + wintun.dll + optional TAP assets)
powershell -ExecutionPolicy Bypass -File scripts\stage-openconnect-runtime-win.ps1 -SourceDir <openconnect-gui install dir>

# 2. Build the Electron desktop bundle (produces both targets in webui/release/)
cd webui
npm install
npm run desktop:build
```

Artifacts (under `webui/release/`):

- `ECNU-VPN-<version>-portable.exe` — single-file portable build; just double-click. No service is installed.
- `ECNU VPN Setup <version>.exe` — NSIS installer; offers per-machine install and automatically registers the `exv-helper` Windows service via `installer.nsh`.

The bundled `bin/` directory contains `exv.exe`, the MinGW runtime DLLs, `openconnect.exe`, GnuTLS / libxml2 / wintun.dll, and (when staged) TAP assets. `libssl-3-x64.dll` and `libcrypto-3-x64.dll` are intentionally excluded — the client no longer links against OpenSSL on Windows.

Both artifacts carry the same native runtime bundle. The runtime must be staged before packaging; otherwise the packaged app will fail with a `runtime_missing` error when attempting to connect VPN.

#### Installer vs Portable — Behavioral Differences

| Aspect | NSIS Installer | Portable |
|--------|---------------|----------|
| **Installation** | Per-machine install to `Program Files` (requires admin) | Single `.exe`, no installation |
| **Service registration** | Offered during setup (UAC already active); runs `exv.exe service install` | Not installed on launch; user must install manually from the Service page |
| **UAC prompts (daily use)** | None after service install | One UAC prompt per VPN session until service is installed |
| **Runtime lookup path** | `%ProgramFiles%\ECNU VPN\bin\` | `<portable-exe-dir>\bin\` (relative to the portable executable) |
| **Shortcuts** | Desktop and Start Menu shortcuts created | None — run the `.exe` directly |
| **Uninstall** | Uninstaller stops + unregisters service, then removes files | Delete the `.exe` — no service cleanup (install service first if needed) |
| **App data** | Preserved on uninstall (`deleteAppDataOnUninstall: false`) | Same — config lives in `%APPDATA%\ecnuvpn\` regardless of artifact type |

### Desktop App (Electron)

The primary interface for EXV is the Electron-based desktop app, which wraps the Vue frontend in a native window. The desktop shell communicates with the native `exv desktop-rpc` JSON interface through Electron IPC — it does not depend on the browser WebUI server.

**Development:**

```bash
# Build the frontend first
cd webui
npm install
npm run build

# Build the Electron main-process code
npm run build:electron

# Package the desktop app (after building the native C++ binary)
npm run desktop:build
```

For live development, build the native binary first (or set `EXV_PATH` to an existing `exv`/`exv.exe`), then run:

```bash
cd webui
npm run desktop:dev
```

## Usage

### VPN Control

| Command | Description |
|---------|-------------|
| `exv` | Start VPN and return to shell |
| `exv stop` / `exv -s` | Stop VPN |
| `exv status` / `exv -t` | Show VPN status and network interfaces |

### Start Options

| Option | Description |
|--------|-------------|
| `-rt [count]` | Auto-reconnect after disconnect (see below) |
| `-f` / `--foreground` | Run WebUI in foreground — compatibility mode (Ctrl+C to stop) |

#### `-rt` Auto-Reconnect

By default (without `-rt`), VPN does not auto-reconnect after a disconnect. Use `-rt` to enable:

```bash
exv -rt          # Reconnect indefinitely until manually stopped
exv -rt -1       # Same as above
exv -rt 3        # Reconnect up to 3 times
exv -rt 0        # No reconnect (same as default)
```

- Only takes effect when starting VPN; cannot be combined with `stop`, `status`, etc.
- Can only be specified once; duplicate specifications cause an error
- A supervisor process forks to monitor openconnect and auto-reconnects on disconnect
- The supervisor exits automatically after the reconnect limit is reached

### Config Management

| Command | Description |
|---------|-------------|
| `exv config` / `exv config show` | Show current config (password masked) |
| `exv config set <key>` | Set a config value interactively |
| `exv config import <file>` | Import config from a JSON file |
| `exv config reset` | Reset to default config (key preserved) |

Configurable keys: `server`, `username`, `password`, `mtu`, `useragent`, `log_file`, `webui_port`, `webui_bind`, `webui_enabled`.

### Route Management

This is the core of the split-tunneling feature. Nine ECNU campus routes are built in by default, and you can add your own:

```bash
# List all split-tunnel routes
exv config routes list

# Add a route (CIDR format, auto-deduplicated)
exv config routes add 10.0.0.0/8

# Remove a route
exv config routes remove 10.0.0.0/8
```

> **Route format**: Use CIDR notation, e.g. `192.168.1.0/24` (network) or `219.228.60.69` (single IP).

### WebUI (Browser Compatibility Mode)

A browser-based WebUI is available for environments where the desktop app is not available (e.g., Linux or headless servers). It provides:

- Real-time VPN status and traffic monitoring
- Config editing
- Real-time log stream
- VPN start/stop control
- Route management

The WebUI does **not** start by default. To launch it, use `exv --webui` (starts VPN + WebUI server) or `exv --webui --foreground` (attached to terminal, Ctrl+C to stop). The WebUI listens at `http://127.0.0.1:18080/` by default.

The desktop app is the recommended interface on macOS and Windows. The WebUI is a compatibility/debugging option.

### Helper Service Management

| Command | Description | Needs sudo/admin |
|---------|-------------|------------------|
| `exv service install` | Install the privileged helper | Yes |
| `exv service uninstall` | Uninstall the privileged helper | Yes |
| `exv service status` | Show helper status | No |

### VPN Modes

- **Helper mode** — The recommended mode for daily use. After installing the privileged helper service (one-time `exv service install`), the desktop app and CLI can start/stop VPN without sudo or admin elevation.
- **Elevated mode** — When the helper service is not installed, the desktop app can use one-time elevation (UAC prompt on Windows, sudo prompt on macOS) for a temporary VPN session. This works for quick use but does not provide persistent convenience like helper mode.
- **Direct mode** — An internal fallback where the desktop app manages VPN directly with elevated privileges. Used automatically when the helper is unavailable and the user grants elevation.

For persistent convenience, install the helper service via the desktop app's Service page or `exv service install`.

## Default Routes

The program ships with built-in routes for ECNU campus resources. When connected, **only traffic to these IPs goes through the VPN tunnel**; all other traffic uses your local network.

View current routes with `exv config routes list`, and add or remove routes as needed.

## Notes

1. **One-time helper installation requires sudo/admin** — `sudo exv service install` (macOS/Linux) or run as Administrator on Windows. After that, `exv` and `exv stop` do not need elevated privileges.

2. **Encrypted password storage** — Passwords are encrypted with AES-256-CBC and stored in `config.json`. The key file (`~/.ecnuvpn/.key` on macOS/Linux, `%APPDATA%\ecnuvpn\.key` on Windows) has permission 0600. `config show` always masks the password.

3. **Route format must be CIDR** — Use CIDR notation (e.g. `10.0.0.0/8`) when adding routes. Single IPs can omit the mask (e.g. `219.228.60.69`). Incorrect formats will cause routes to not take effect.

4. **VPN server route auto-protection** — The program automatically detects whether the VPN server IP is covered by a split-tunnel route. If so, it adds a host route pointing to the default gateway to prevent the VPN connection itself from being swallowed by the tunnel ("split-brain" problem).

5. **openconnect must be installed** — On macOS, the program will prompt to install via Homebrew if openconnect is missing.

6. **WebUI listens on localhost by default** — Default binding is `127.0.0.1:18080`, accessible only from the local machine. To allow LAN access, change `webui_bind` to `0.0.0.0` (note the security implications).

7. **Foreground mode is for debugging/compatibility** — `exv -f` runs the WebUI in the foreground (compatibility mode); Ctrl+C stops it. The default CLI behavior is start VPN and return to shell.

8. **Uninstall helper before removing the program** — Run `sudo exv service uninstall` (or the Windows equivalent) to clean up the helper daemon before uninstalling.

## FAQ

**Q: Internet is slow after connecting to VPN?**

This is the problem EXV solves. Make sure you are using `exv` instead of the native AnyConnect client. `exv` uses split tunneling to route only campus traffic through the VPN. Run `exv config routes list` to verify your route configuration.

**Q: VPN fails to start, says openconnect not installed?**

The program will ask: `Install openconnect now? [Y/n]` — press Enter to install via Homebrew. You can also manually run `brew install openconnect`.

**Q: VPN is connected but campus resources are inaccessible?**

Check your route configuration: `exv config routes list`, and confirm the target IP range is included. If the campus IP you need is not in the default route table, add it manually.

**Q: Prompt says "helper daemon is not installed"?**

Run `sudo exv service install` to install the helper. After installation, `exv` and `exv stop` do not need sudo.

**Q: Password shows `[KEY MISSING]` or `[KEY CORRUPT]`?**

The key file is corrupted. Regenerate it:

```bash
exv config key reset    # Regenerate key (existing password ciphertext will be cleared)
exv config set password # Set password again
```

**Q: How to add a new campus route?**

```bash
exv config routes add 202.120.96.0/19
```

The route takes effect the next time VPN is started. You can also add routes through the desktop app or WebUI.

**Q: `exv stop` says process not found?**

The PID file may be lost; the helper falls back to `pgrep` to find it. If that also fails, you can manually run `sudo killall openconnect`.

## Configuration

Config file location: `~/.ecnuvpn/config.json` on macOS/Linux, `%APPDATA%\ecnuvpn\config.json` on Windows.

```json
{
    "server": "https://vpn-ct.ecnu.edu.cn",
    "username": "20XXXXXXXXX",
    "password": "<AES-256-CBC ciphertext>",
    "mtu": 1290,
    "useragent": "AnyConnect Darwin_x86_64 4.10.05095",
    "routes": ["49.52.4.0/25", "..."],
    "extra_args": [],
    "log_file": "~/.ecnuvpn/ecnuvpn.log",
    "webui_port": 18080,
    "webui_bind": "127.0.0.1",
    "webui_enabled": false
}
```

When importing via `config import`, the `password` field can be plaintext — the program will encrypt it automatically.

## Tech Stack

- **C++17** + CMake
- **openconnect** — VPN connection core
- **nlohmann/json** — JSON parsing
- **cpp-httplib** — Embedded HTTP server
- **Vue 3 + TypeScript + Vite** — Frontend (shared by desktop app and browser WebUI)
- **Electron** — Desktop app shell (macOS / Windows)
- **CommonCrypto** (macOS) / **OpenSSL** (Linux) / **CNG BCrypt** (Windows) — AES-256-CBC encryption
- **launchd** (macOS) / **Windows Service** (Windows) / **systemd** (Linux) — Privileged helper management

## License

[MIT](LICENSE)
