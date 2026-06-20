# User Guide

EXV provides a native WebView desktop shell backed by the C++ core and privileged helper. The desktop shell is the recommended user interface for connecting, disconnecting, inspecting status, and editing configuration.

## Launching The Desktop App

Build or install the native WebView package for your platform, then launch `exv-ui` from:

```text
build/<platform>/webview/package/EXV
```

Windows development shortcut:

```powershell
.\start.ps1
```

Package-only build:

```powershell
.\start.ps1 -Package
```

## Package Contents

- `exv-ui` or `exv-ui.exe`: desktop shell.
- `exv-ui.args`: packaged launch arguments.
- `bin/exv`: core process.
- `bin/exv-helper`: privileged helper.
- `webui/index.html`: renderer entry.

Windows packages also include `WebView2Loader.dll`. If the WebView2 Evergreen Runtime is missing, the desktop shell reports the missing runtime and uses the controlled install flow after user consent.

## Helper Modes

The helper owns privileged operations such as service maintenance, DNS updates, route changes, tunnel adapter creation, and cleanup. Core owns business state and asks helper to execute privileged work through the helper contract.

Two helper lifecycle modes exist:

- one-shot helper: launched by core when privileged work is needed and bound to the current core process lease.
- service helper: managed by the OS service manager and reused across app launches.

The UI can inspect helper status through the core RPC surface. It should distinguish service installation state from the currently active helper instance.

## Connecting

1. Open the native WebView desktop shell.
2. Confirm server, username, and routing settings.
3. Start the connection.
4. Approve helper elevation if prompted.

After the helper is elevated, subsequent privileged operations in the same core lifecycle use that helper instead of prompting again.

## Disconnecting

Disconnect requests go through core to helper. Helper removes routes, restores DNS, cleans tunnel resources, and reports completion. In the one-shot mode, helper remains available for the current core lease after VPN disconnect; it exits when core releases the lease or heartbeats time out.

## Service Install And Uninstall

- Installing the service is a helper-owned privileged task.
- Service install can run while a one-shot helper is active.
- Service uninstall requires no active VPN session. Core must disconnect first, then ask the current helper to uninstall.

If a one-shot helper is already elevated, service maintenance can reuse it so the user is not asked for another elevation prompt.

## Configuration

Default config locations:

- Windows: `%LOCALAPPDATA%\EXV\profile\default\config.json`
- macOS: `~/Library/Application Support/EXV/profile/default/config.json`
- Linux: `~/.exv/config.json`

Config stores user intent and preferences. Runtime facts such as active adapters, routes, DNS snapshots, cleanup plans, and helper lease state belong to helper/core runtime state, not the config file.

## Browser WebUI Compatibility

The browser WebUI is kept as a compatibility and diagnostics entry. Production desktop packaging uses the native WebView shell at:

```text
build/<platform>/webview/package/EXV
```
