# Windows Electron Helper Recovery Notes

## Background

This note records the Windows-side recovery work that made the Electron UI,
native RPC bridge, and privileged helper service usable together. The debugging
path was unusually expensive because several symptoms looked like missing DLLs
or administrator-permission problems, while the real failures were split across
runtime staging, Windows service lifecycle, named pipe IPC, and Electron payload
serialization.

The practical goal was:

- `pnpm run desktop:dev` can launch a real Electron UI against the current native
  binaries.
- The packaged desktop app can stage the same native runtime.
- A normal logged-in user can connect through a one-time installed helper
  service instead of running the UI as Administrator every time.
- Service install/uninstall and diagnostics give actionable state instead of a
  silent spinner.

## Failure Timeline

### 1. Native executable could not start

The first visible Windows blocker was `STATUS_DLL_NOT_FOUND`. `exv.exe` had been
built with MinGW GCC, so it needed MinGW runtime DLLs next to the executable:

- `libgcc_s_seh-1.dll`
- `libstdc++-6.dll`
- `libwinpthread-1.dll`

The repository already had the required runtime inputs in multiple places, but
the Electron release staging area had drifted. Some old packaged binaries also
pulled in outdated runtime dependencies. The fix was to make the development and
packaging path prefer the current `build/exv.exe` and stage the runtime assets
from `runtime/win32-x64`.

### 2. Electron IPC could not clone Vue payloads

Saving authentication settings failed with:

```text
An object could not be cloned.
```

The renderer was passing Vue reactive/proxy objects through Electron IPC. The
desktop IPC boundary now receives plain JSON-shaped objects for credentials,
settings, routes, and related payloads. This keeps the renderer state model
separate from the structured clone requirements of Electron.

### 3. Helper install appeared successful but helper was unavailable

The service could be registered and SCM reported it as installed, but connection
requests still failed with:

```text
Helper daemon is not available. Install the helper service from Settings or run
'exv service install' as Administrator.
```

The original Windows service model registered the hidden helper mode in the
main CLI executable as a Windows service. That process did not implement the normal Windows service
lifecycle handshake with SCM. A proper Windows service process must call
`StartServiceCtrlDispatcher`, enter `ServiceMain`, register a control handler,
and report `SERVICE_RUNNING`. Without that handshake SCM can wait for service
startup and eventually report service timeout errors such as 1053.

The fix was to split the service entry point:

- `exv.exe` remains the user-facing CLI/native RPC binary.
- `exv-helper.exe` is the dedicated Windows service executable.
- The registered service binary path is now:

```text
"C:\Program Files\ECNU-VPN\exv-helper.exe" --service
```

The helper service is staged to a stable install directory before registration,
then started by SCM.

### 4. Named pipe protocol blocked and then denied normal users

The helper IPC protocol is newline-delimited JSON. The Windows pipe server was
reading until pipe close instead of stopping at `\n`, so clients could write a
request and wait for a response while the server kept waiting for the client to
close the pipe. This made `status` probes and install readiness checks slow and
unreliable.

The server now stops reading at the first newline, sends the response, flushes
the pipe, and disconnects the client explicitly.

After the service split, the pipe existed but ordinary desktop clients still saw
`available=false`. The service runs as LocalSystem, while the Electron UI runs as
the interactive user. The named pipe now uses an explicit DACL so interactive
users can connect, and peer verification trusts the pipe ACL on Windows instead
of depending on fragile impersonation for the normal UI path.

### 5. Service operations needed visible progress and accurate status

The UI originally treated service install/uninstall as a single opaque action.
That made a stuck readiness probe look like a frozen application.

The Electron main process now streams elevated command output as service
progress events. The Service page shows operation output and refreshes status
after install/uninstall. Status distinguishes:

- `installed`
- `running`
- `available`
- service binary path
- SCM state

Install no longer claims full success if the service is installed but the pipe is
not responsive.

### 6. Logs crashed JSON serialization on non-UTF-8 bytes

The packaged UI later failed while loading logs:

```text
nlohmann::json type_error.316 invalid UTF-8 byte
```

Windows logs can contain bytes that are valid in the local ANSI code page but not
valid UTF-8. `nlohmann::json` validates strings during serialization, so raw log
lines could terminate the native RPC process. Log lines are now sanitized before
being returned to Electron.

### 7. Elevated RPC cannot depend on stdout or process lifetime

The no-service Windows path originally tried to run elevated native RPC through
PowerShell and read stdout after `Start-Process -Verb RunAs -Wait`. This broke in
two different ways:

- JSON payloads passed through nested PowerShell quoting could arrive corrupted.
- The elevated wrapper could keep waiting even after the native command had
  already produced a valid result, because the one-shot helper and OpenConnect
  outlived the short RPC process.

The stable pattern is now:

```text
normal Electron main
  -> write payload JSON file
  -> launch elevated PowerShell script
  -> elevated exv.exe writes result JSON file directly
  -> normal Electron main polls the result file
```

Do not reintroduce stdout capture for Windows elevated desktop RPC. Result files
are the handoff boundary; the elevated process lifetime is not the completion
signal.

### 8. OpenConnect `--script` is unreliable in the hidden elevated chain

OpenConnect reached CSTP/DTLS, but its Windows `--script` child launch failed
with `句柄无效` for all tested script hosts:

- `cscript.exe`
- `wscript.exe`
- `exv-helper.exe __tunnel-script`

That means the failure was not JavaScript-specific. It was the hidden elevated
process chain and inherited handle environment. The connection could appear
established while Windows routes and interface configuration were still missing.

The current Windows workaround parses the OpenConnect runtime log for:

- assigned internal IP
- Wintun adapter name
- adapter index

Then native code configures the interface and routes and writes `route-ready`.
Status must treat `connected` as `process_running && network_ready`; a running
OpenConnect process alone is not enough.

### 9. One-shot mode must keep its backend identity in the UI

After a successful one-shot connection, the first `connectElevated` response has
the temporary backend endpoint and token. A later plain `status.get` can only see
the OpenConnect process and route marker; it cannot rediscover the one-shot
auth token. If the UI overwrites the session with that plain status, it loses the
fact that this is an elevated temporary connection.

The observed failure mode was:

```text
connectElevated succeeds
status polling overwrites mode/backend
UI thinks the session is helper/service managed
disconnect calls normal ecnu-vpn:rpc
normal RPC fails with helper_unavailable
```

The store now preserves the temporary backend and infers `mode=elevated` while
connected without a running service. Disconnect for elevated/direct sessions must
use the elevated path, even if the backend has been lost; the native fallback can
clean up OpenConnect and the one-shot helper.

### 10. Service status is not helper status

A running `exv-helper.exe --oneshot` process is not an installed Windows
service. The service page and service status must be based on SCM state only:

```powershell
sc.exe query exv-helper
```

If SCM returns 1060, the service is not installed, even if a temporary helper
process is currently serving a one-shot VPN session. Keep these concepts
separate in UI copy and state:

- service installed/running/available
- one-shot helper running for the current session
- OpenConnect process running
- network ready

### 11. Connection progress should expose phases

Showing only `等待授权` makes the user think the app is still waiting for UAC,
even after authorization has succeeded and the client is actually creating the
adapter, connecting to the VPN server, or writing routes. The UI now separates
the temporary connection flow into visible phases:

- waiting for authorization
- starting one-shot helper
- connecting to VPN server
- creating virtual adapter
- writing routes
- waiting for network readiness

Future native progress events should replace the current UI-side phase estimate,
but the user-facing model should remain multi-stage. These phases are also good
log markers for future debugging.

### 12. Virtual-adapter warnings are connected-state context

The app can detect upstream virtual adapters such as Mihomo even while
disconnected. Showing that warning before a connection is active makes it look
like EXV has already changed networking. The dashboard should only show the
upstream virtual adapter warning while connected.

## Current Windows Architecture

```text
Electron UI
  |
  | desktop IPC
  v
Electron main process
  |
  | child_process exv.exe desktop-rpc <action> <json>
  v
exv.exe
  |
  | named pipe \\.\pipe\exv-helper
  v
exv-helper.exe --service
  |
  | privileged VPN/runtime operations
  v
OpenConnect / Wintun / Windows networking
```

The important boundary is that `exv.exe` is no longer the Windows service
process. Service lifecycle belongs to `exv-helper.exe`; desktop RPC and CLI
remain in `exv.exe`.

## Verification Commands

Run these from an Administrator PowerShell after rebuilding:

```powershell
cd "D:\Development\Projects\cpp\ECNU-VPN"

.\build\exv.exe service uninstall
.\build\exv.exe service install
.\build\exv.exe service status
.\build\exv.exe desktop-rpc service.status "{}"
```

Expected status:

```text
Installed       : yes
State           : running
Socket Ready    : yes
```

Expected JSON contains:

```json
{
  "installed": true,
  "running": true,
  "available": true
}
```

For development UI testing:

```powershell
cd "D:\Development\Projects\cpp\ECNU-VPN\webui"

$env:EXV_PATH="D:\Development\Projects\cpp\ECNU-VPN\build\exv.exe"
$env:EXV_HELPER_PATH="D:\Development\Projects\cpp\ECNU-VPN\build\exv-helper.exe"
$env:ECNUVPN_RUNTIME_DIR="D:\Development\Projects\cpp\ECNU-VPN\runtime\win32-x64"

pnpm run desktop:dev
```

For packaged UI testing:

```powershell
cd "D:\Development\Projects\cpp\ECNU-VPN\webui"
pnpm run desktop:build
& "D:\Development\Projects\cpp\ECNU-VPN\webui\release\win-unpacked\ECNU-VPN.exe"
```

Before rebuilding the packaged UI, close all running `ECNU-VPN.exe` instances so
Electron Builder can replace `release/win-unpacked`.

## Maintenance Notes

- Do not register the old hidden helper mode in the main CLI executable as the Windows service again. The
  service entry point must remain a proper SCM-compatible executable.
- If helper availability regresses, check both SCM state and pipe readiness. A
  service can be `RUNNING` while the desktop client still cannot talk to the
  named pipe.
- Administrator UI is not a substitute for a working helper. The design goal is
  one-time privileged service installation followed by ordinary user operation.
- Keep runtime staging deterministic. The release bundle and stable helper
  install directory must include `exv.exe`, `exv-helper.exe`, MinGW runtime DLLs,
  OpenConnect runtime files, and `wintun.dll`.
- Treat all Electron IPC payloads as plain data. Do not pass Vue reactive objects
  or class instances across the IPC bridge.
- Treat log file bytes as untrusted text. Sanitize or decode before JSON
  serialization.
