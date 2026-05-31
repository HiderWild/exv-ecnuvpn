# EXV - ECNU Smart VPN Client

> Native ECNU VPN client with split tunneling, encrypted credential storage, privileged helper service, and desktop/WebUI interfaces.

## What EXV Does

Cisco AnyConnect commonly sends all traffic through the campus VPN after connection. EXV keeps only campus traffic on the VPN tunnel and leaves normal internet traffic on the local network.

- Campus IP ranges route through the VPN.
- Non-campus traffic keeps using the local default route.
- Credentials are encrypted locally.
- The desktop app is the recommended daily-use interface.

## Production Runtime

EXV now uses the native VPN implementation as the production default on supported platforms. Production desktop packages do not require OpenConnect or Homebrew OpenConnect.

OpenConnect is kept only as a development or diagnostic fallback for comparing behavior with the legacy backend. Do not document or treat it as a production runtime dependency.

Native mode does not support arbitrary OpenConnect-style `extra_args`. Use the supported EXV configuration keys and route management commands instead.

## Install

### Recommended: Desktop App

1. Download the macOS or Windows desktop package from the release artifacts.
2. Install or launch the app.
3. Use the Service page to install the privileged helper once.
4. Enter your ECNU campus credentials and connect.

### Build from Source

Use the platform wrapper scripts so native, frontend, and desktop artifacts stay under the platform-specific output locations.

```bash
git clone <repo>
cd ECNU-VPN
```

macOS:

```bash
./scripts/build-macos.sh all
./scripts/build-macos.sh desktop
```

Windows:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action all
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action desktop
```

Native-only builds remain available for focused development:

```bash
cmake --preset macos-release
cmake --build --preset macos-release
ctest --preset macos-release -R 'platform_status_models_test|vpn_runtime_test'
```

```powershell
cmake --preset windows-release
cmake --build --preset windows-release --target exv exv-helper platform_status_models_test vpn_runtime_test
ctest --preset windows-release -R 'platform_status_models_test|vpn_runtime_test'
```

## Build Output Layout

- `build-windows/cpp` - Windows native CMake output, tests, and `compile_commands.json`
- `build/windows/electron/dist` - Windows renderer bundle
- `build/windows/electron/dist-electron` - Windows Electron main/preload bundle
- `build/windows/electron/native/bin` - Windows packaged native payload
- `build/windows/electron/release` - Windows installer and portable output
- `build/macos/cpp` - macOS native CMake output, tests, and `compile_commands.json`
- `build/macos/electron/dist` - macOS renderer bundle
- `build/macos/electron/dist-electron` - macOS Electron main/preload bundle
- `build/macos/electron/native/bin` - macOS packaged native payload
- `build/macos/electron/release` - macOS desktop package output

## Runtime Assets

Windows native production packages stage:

- `exv.exe`
- `exv-helper.exe`
- required MinGW runtime DLLs
- `wintun.dll`

The Windows native runtime asset is `wintun.dll`. Production packages do not require staging legacy OpenConnect runtime files.

macOS native production packages bundle the native `exv` binary and helper integration. Homebrew OpenConnect is not required for native production packages.

## Quick Start

```bash
# 1. Set username
exv config set username

# 2. Set password; input is hidden and stored encrypted
exv config set password

# 3. Install the privileged helper once
sudo exv service install

# 4. Start VPN
exv

# 5. Stop VPN
exv stop
```

Configuration lives under `~/.ecnuvpn/` on macOS/Linux and `%APPDATA%\ecnuvpn\` on Windows.

## Common Commands

### VPN

| Command | Description |
|---------|-------------|
| `exv` | Start VPN with split tunneling |
| `exv stop` / `exv -s` | Stop VPN |
| `exv status` / `exv -t` | Show VPN status |

### Auto-Reconnect

```bash
exv -rt          # enable auto-reconnect with default retry behavior
exv -rt -1       # retry indefinitely
exv -rt 3        # retry up to 3 times
exv -rt 0        # disable auto-reconnect
```

### Configuration

| Command | Description |
|---------|-------------|
| `exv config` / `exv config show` | Show current configuration |
| `exv config set <key>` | Set a configuration value |
| `exv config import <file>` | Import JSON configuration |
| `exv config reset` | Reset configuration after confirmation |
| `exv config routes list` | List split-tunnel routes |
| `exv config routes add <cidr>` | Add a split-tunnel route |
| `exv config routes remove <cidr>` | Remove a split-tunnel route |

Supported production configuration uses explicit EXV keys such as `server`, `username`, `password`, `mtu`, `useragent`, `routes`, `log_file`, `webui_port`, `webui_bind`, and `webui_enabled`. Native mode does not accept arbitrary `extra_args`.

## Desktop UI

The Electron desktop app is the recommended interface. It talks to the native `exv desktop-rpc` JSON interface through Electron IPC and does not depend on the browser WebUI server.

The browser WebUI remains available as a compatibility entry point through foreground mode:

```bash
exv -f
```

See [docs/user_guide.md](docs/user_guide.md) for daily-use instructions and [docs/build_guide.md](docs/build_guide.md) for the full build matrix.

## License

[MIT](LICENSE)
