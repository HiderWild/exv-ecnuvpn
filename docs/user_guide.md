# EXV User Guide

> Native ECNU VPN client with split tunneling, encrypted credential management, privileged helper service, and desktop/WebUI interfaces.
>
> Current version: **v3.3.0** | macOS / Windows / Linux | Native production runtime | Service installation recommended for daily use

---

## Installation

### Desktop App (Recommended)

The easiest way to get started is with the **desktop app** (macOS / Windows):

1. Download the latest release for your platform from the Releases page.
2. **macOS**: Drag to Applications, then run. On first launch, grant admin access to install the privileged helper.
3. **Windows**: Run the NSIS installer (or the portable `.exe`). On first launch, accept the UAC prompt to install the `exv-helper` Windows service.
4. Enter your campus username and password and click Connect.

The desktop app communicates with the native `exv desktop-rpc` JSON interface through Electron IPC. It does not depend on the browser WebUI server.

### Building from Source

The project has a strict build dependency chain: **frontend build -> native build -> desktop build**. The frontend must be built first because `scripts/embed_assets.py` reads `webui/dist/` during the C++ build.

```bash
git clone <repo>
cd ECNU-VPN

# 1. Build the frontend first
cd webui
pnpm install
pnpm run build
cd ..
```

Then build the native binary with the preset for your platform.

macOS:

```bash
cmake --preset macos-release
cmake --build --preset macos-release
```

Windows:

```powershell
cmake --preset windows-release
cmake --build --preset windows-release
```

Linux:

```bash
cmake --preset linux-release
cmake --build --preset linux-release
```

#### Platform-Specific Prerequisites

| Platform | Prerequisites |
|----------|--------------|
| macOS | CMake 3.28+, Ninja 1.11+, Homebrew LLVM/Clang, Xcode command line tools, Node.js; Homebrew OpenConnect is not required for native production packages |
| Linux (Ubuntu/Debian) | CMake 3.28+ plus `sudo apt install libssl-dev ninja-build gcc-14 g++-14 build-essential` |
| Linux (Fedora/RHEL) | CMake 3.28+ plus `sudo dnf install openssl-devel ninja-build gcc-c++` with a module-capable GCC/Clang toolchain |
| Windows | CMake 3.28+, Ninja 1.11+, MSVC from Visual Studio 2022 or newer, Node.js; openconnect-gui is not required for native production packages |

All platforms also require **Node.js** (v18+) for the frontend build step. Native builds use C++20 and include a helper protocol named-module smoke test, so the C++ compiler must support CMake C++ module dependency scanning. OpenConnect may be installed separately only when you are developing or diagnosing the legacy fallback backend; it is not part of the production runtime path.

### Build Directory Convention

To reduce artifact conflicts between the `windows` and `macos` branches before merge, build output is separated into:

- `build-windows/cpp`
- `build/windows/electron/*`
- `build/macos/cpp`
- `build/macos/electron/*`

Recommended: use the platform scripts directly:

```bash
# macOS
./scripts/build-macos.sh all

# Windows
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action all
```

### Installing the Privileged Helper (One-Time)

```bash
# macOS / Linux
sudo exv service install

# Windows (run as Administrator)
exv.exe service install
```

This installs the privileged helper daemon (launchd on macOS, Windows service on Windows, systemd helper on Linux). After installation, VPN connect/disconnect operations work without repeated elevation prompts. Installing the global `exv` terminal command is a separate optional setting in the desktop app.

---

## Quick Start

### Using the Desktop App

1. Launch the EXV desktop app.
2. On first run, enter your campus username and password.
3. Click **Connect**. The app will handle VPN startup, split-tunnel routing, and reconnection.
4. Use the **Routes** tab to add or remove campus IP ranges.
5. Check the **Logs** tab for real-time connection status.

### Using the CLI

First-time setup:

```bash
# 1. Set username
exv config set username
# > Enter value for username: 20XXXXXXXXX

# 2. Set password (hidden input, encrypted storage)
exv config set password
# >   New password: ••••••••
# >   Confirm password: ••••••••

# 3. Install the helper (one-time, requires sudo/admin)
sudo exv service install

# 4. Start VPN (no sudo needed after helper is installed)
exv
```

If you use the desktop app and have not installed the global CLI, the packaged `exv` binary is still available inside the app resources. Install the global CLI from Settings only when you want `exv` to resolve from a normal terminal.

On first run, the config file and encryption key are created automatically at:
- **macOS / Linux**: `~/.ecnuvpn/`
- **Windows**: `%APPDATA%\ecnuvpn\`

---

## Command Reference

```
exv [command] [subcommand] [args]
```

### VPN Control

| Command | Description | Needs sudo/admin |
|---------|-------------|------------------|
| `exv` | Start VPN and return to shell | No (after helper install) |
| `exv stop` \| `-s` | Stop VPN | No (after helper install) |
| `exv status` \| `-t` | Show VPN status and network interfaces | No |

### Start Options

| Option | Description |
|--------|-------------|
| `-rt [count]` | Auto-reconnect count after disconnect (omit or -1 for unlimited) |
| `-f`, `--foreground` | Run WebUI in foreground (compatibility mode; Ctrl+C to stop) |

### Helper Service Management

| Command | Description | Needs sudo/admin |
|---------|-------------|------------------|
| `exv service install` | Install the privileged helper | Yes |
| `exv service uninstall` | Uninstall the privileged helper | Yes |
| `exv service status` | Show helper status | No |

On macOS, `service install` registers the launchd helper and manages `/usr/local/bin/exv-helper`; it does not install the global `/usr/local/bin/exv` CLI command. On Windows, it registers the `exv-helper` Windows service. On Linux, it installs a systemd helper.

### Optional Global CLI

The desktop app always bundles the native `exv` program in its resources so it can be run by full path. Installing the global CLI only adds a convenient terminal entry point:

| Platform | Install behavior |
|----------|------------------|
| Windows | Creates a user-level `exv.cmd` shim under `%LOCALAPPDATA%\ECNU-VPN\cli` and adds that directory to the user PATH. |
| macOS | Creates `/usr/local/bin/exv` as a symlink to the packaged `exv` binary. |

Use **Settings -> Terminal CLI** to install or uninstall this global command. Helper service installation and CLI installation are independent.

### Config Management

| Command | Description |
|---------|-------------|
| `exv config` \| `config show` | Show current config (password masked) |
| `exv config set <key>` | Set a config value interactively |
| `exv config import <file>` | Import config from a JSON file |
| `exv config reset` | Reset to default config (key preserved) |

**Configurable keys:**

| Key | Description |
|-----|-------------|
| `server` | VPN server address |
| `username` | Login username (student ID) |
| `password` | Login password (hidden input, encrypted storage) |
| `mtu` | MTU value (default 1290) |
| `useragent` | User-Agent string |
| `log_file` | Log file path |
| `webui_port` | WebUI port (default 18080) |
| `webui_bind` | WebUI bind address (default 127.0.0.1) |
| `webui_enabled` | Enable WebUI (default true) |

### Route Management

| Command | Description |
|---------|-------------|
| `exv config routes list` | List all split-tunnel routes |
| `exv config routes add <cidr>` | Add a route (auto-deduplicated) |
| `exv config routes remove <cidr>` | Remove a route |

### Key Management

| Command | Description |
|---------|-------------|
| `exv config key show` | Show key file path and validity |
| `exv config key reset` | Regenerate key (clears password ciphertext, requires confirmation) |

### Logs and Help

| Command | Description |
|---------|-------------|
| `exv logs` \| `-l` | Show last 50 log entries |
| `exv help` \| `-h` | Help information |
| `exv version` \| `-v` | Version number |

---

## Desktop App

The Electron-based desktop app is the recommended interface for macOS and Windows. It provides:

- One-click VPN connect/disconnect
- Real-time VPN status and traffic monitoring
- Route management (add/remove campus IP ranges)
- Config editing
- Real-time log viewer
- No browser required

The desktop shell communicates with the native `exv desktop-rpc` JSON interface through Electron IPC (preload script). It does not open or depend on the embedded WebUI HTTP server.

### Desktop App Development

```bash
cd webui
pnpm install
pnpm run build
pnpm run build:electron

# Package the desktop app (after building the native C++ binary)
pnpm run desktop:build
```

For live development, build the native binary first or set `EXV_PATH`:

```bash
cd webui
pnpm run desktop:dev
```

### Windows Desktop Packaging

The Electron desktop app can be packaged for Windows in two flavours:

```powershell
# Build the Electron desktop bundle (produces both targets in webui/release/)
cd webui
pnpm install
pnpm run desktop:build
```

Artifacts (under `webui/release/`):

- `ECNU-VPN-<version>-portable.exe` — single-file portable build; just double-click. No service is installed.
- `ECNU VPN Setup <version>.exe` — NSIS installer; offers per-machine install and automatically registers the `exv-helper` Windows service.

Neither Windows artifact adds `exv` to PATH by default. Use Settings -> Terminal CLI to install or remove the global command explicitly.

Windows native production packages include the native `exv`/`exv-helper` binaries, required MinGW runtime DLLs, and `wintun.dll`. `wintun.dll` is the Windows native runtime asset. Production packaging no longer requires staging legacy OpenConnect runtime files.

### Desktop App VPN Modes

The desktop app can operate in three modes depending on whether the privileged helper service is installed:

- **Helper mode** — The recommended mode for daily use. After installing the helper service (one-time step via the desktop app's Service page or `exv service install`), the desktop app and CLI can start/stop VPN without sudo or admin elevation. All operations go through the helper daemon (launchd on macOS, Windows service on Windows, systemd on Linux).

- **Elevated mode** — When the helper service is not installed, the desktop app can use one-time elevation (UAC prompt on Windows, sudo prompt on macOS) for a temporary VPN session. This works for quick use but does not provide persistent convenience like helper mode. The desktop app handles the elevation flow automatically.

- **Direct mode** — An internal fallback where the desktop app manages VPN operations directly with elevated privileges. Used automatically when the helper is unavailable and the user has granted elevation. This mode does not persist across app restarts.

For persistent convenience, install the helper service via the desktop app's Service page or `exv service install`.

---

## WebUI (Browser Compatibility Mode)

A browser-based WebUI is available for environments where the desktop app is not available (e.g., Linux, or running on a headless server), providing:

- Real-time VPN status and traffic monitoring
- Config editing
- Real-time log stream (SSE)
- VPN start/stop control
- Route management

The WebUI starts by default when VPN is launched. To disable it: `exv config set webui_enabled` set to `false`. To run in foreground: `exv --webui --foreground` (Ctrl+C to stop). The WebUI listens at `http://127.0.0.1:18080/` by default.

The WebUI is a **compatibility/debugging option**. The desktop app is the recommended interface on macOS and Windows.

---

## Config File Format

Config file location: `~/.ecnuvpn/config.json` on macOS/Linux, `%APPDATA%\ecnuvpn\config.json` on Windows.

```json
{
    "server": "https://vpn-cn.ecnu.edu.cn",
    "username": "20XXXXXXXXX",
    "password": "<AES-256-CBC ciphertext>",
    "mtu": 1290,
    "useragent": "AnyConnect Darwin_x86_64 4.10.05095",
    "routes": [
        "49.52.4.0/25",
        "59.78.176.0/20"
    ],
    "log_file": "~/.ecnuvpn/ecnuvpn.log",
    "webui_port": 18080,
    "webui_bind": "127.0.0.1",
    "webui_enabled": true
}
```

When importing via `config import`, the `password` field can be plaintext — the program will encrypt it automatically.

Native production mode does not support arbitrary OpenConnect-style `extra_args`. Configure EXV through the documented keys above and the route management commands. Legacy OpenConnect arguments are only relevant when explicitly running development or diagnostic fallback flows.

---

## Encryption

- Passwords are encrypted with **AES-256-CBC**
  - macOS: CommonCrypto (built-in, no extra dependency)
  - Windows: CNG/BCrypt (built-in, no extra dependency)
  - Linux: OpenSSL
- The encryption key (32-byte random) is stored at `~/.ecnuvpn/.key` (macOS/Linux) or `%APPDATA%\ecnuvpn\.key` (Windows) with file permission **0600**
- `config show` always displays a masked result (`••••••••  (encrypted)`)

### Key Troubleshooting

If `config show` displays `[KEY MISSING]` or `[KEY CORRUPT]`:

```bash
exv config key reset    # Regenerate key (existing password ciphertext will be cleared)
exv config set password # Set password again
```

---

## Runtime Files

### User-Level Files

| File | Description |
|------|-------------|
| `~/.ecnuvpn/config.json` (or `%APPDATA%\ecnuvpn\config.json`) | Config (password is ciphertext) |
| `~/.ecnuvpn/.key` (or `%APPDATA%\ecnuvpn\.key`) | AES-256 encryption key (0600) |
| `~/.ecnuvpn/tunnel.sh` | Tunnel script (auto-generated on each start) |
| `~/.ecnuvpn/ecnuvpn.pid` | VPN session process PID |
| `~/.ecnuvpn/ecnuvpn-supervisor.pid` | Auto-reconnect supervisor PID |
| `~/.ecnuvpn/route-ready` | Route configuration completion marker (interface name + internal IP) |
| `~/.ecnuvpn/ecnuvpn.log` | Timestamped runtime log |

### System-Level Files (macOS)

| File | Description |
|------|-------------|
| `/Library/LaunchDaemons/com.ecnu.exv.helper.plist` | launchd root helper definition |
| `/var/run/exv-helper.sock` | Local Unix socket (root:staff 0660), used by regular users to request the root helper |
| `/var/run/exv-helper-session.json` | Current session state managed by the helper |

### System-Level Files (Windows)

| File | Description |
|------|-------------|
| `C:\Program Files\ECNU-VPN\bin\exv.exe` | Native VPN binary |
| `exv-helper` Windows service | Registered via `exv service install` |
| `%APPDATA%\ecnuvpn\` | User config, key, and log files |

---

## FAQ

**Q: A legacy diagnostic fallback says OpenConnect is not installed?**

Native production mode does not require OpenConnect. Install OpenConnect only if you are intentionally running a development or diagnostic fallback comparison.

**Q: VPN is connected but campus resources are inaccessible?**

Check your route configuration: `exv config routes list`, and confirm the target IP range is included.

**Q: Why don't I need `sudo exv` every time?**

After `sudo exv service install` (or the Windows equivalent), `exv` / `exv stop` communicate with the privileged helper through a Unix socket (macOS/Linux) or named pipe (Windows) to perform privileged operations on your behalf.

**Q: `exv` says "helper daemon is not installed"?**

Run `sudo exv service install` to install the helper. After installation, `exv` and `exv stop` do not need sudo.

**Q: `exv stop` says process not found?**

The PID file may be lost; the helper falls back to platform process discovery. If that also fails, stop the native VPN session from the desktop app, service manager, or Task Manager. For legacy diagnostic fallback sessions only, stop the OpenConnect process manually.
