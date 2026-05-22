# Manual Test Guide — Develop Merge Readiness

> Status: PENDING USER EXECUTION
> Date: 2026-05-22
> Branch: `integration/platform-convergence-next`

---

## R1: Windows Develop Gate

### R1.1 Automated Validation [develop-blocker]

Open an Administrator PowerShell and run:

```powershell
cd "D:\Development\Projects\cpp\ECNU-VPN"
git switch integration/platform-convergence-next
git status --short --branch
```

Confirm: branch is `integration/platform-convergence-next`, working tree clean.

```powershell
powershell -ExecutionPolicy Bypass -File ".\scripts\validate-merge-prep-windows.ps1"
```

Success indicators (in order):
- `[merge-prep] Build frontend assets for native embedding...` completes without error
- `[merge-prep] Configure and build native targets...` completes without error
- `[merge-prep] Run focused native regression tests...` — all 5 tests pass
- `[merge-prep] Compile Electron main/preload and native staging...` completes without error
- `[merge-prep] Windows validation complete.` printed at the end

If a step fails: record the exact error output. Do not proceed to R1.2 until R1.1 passes.

Next, build the desktop package:

```powershell
cd "D:\Development\Projects\cpp\ECNU-VPN\webui"
npm run desktop:build
```

Success: `electron-builder` produces a packaged output under `webui/dist/` or `webui/release/` without errors.

Verify the package contains expected binaries:

```powershell
$pkgDir = Get-ChildItem -Path "D:\Development\Projects\cpp\ECNU-VPN\webui\release" -Directory | Select-Object -First 1
Get-ChildItem -Path $pkgDir.FullName -Recurse -Include "exv.exe","exv-helper.exe","wintun.dll" | Select-Object FullName
```

Success: all three files found. Also check for MinGW runtime DLLs and OpenConnect assets:

```powershell
Get-ChildItem -Path $pkgDir.FullName -Recurse -Include "*.dll","openconnect*" | Select-Object FullName
```

Success: MinGW runtime DLLs and OpenConnect runtime assets present.

---

### R1.2 Windows Administrator GUI Service Path [develop-blocker]

Prerequisite: R1.1 passed. Run from an Administrator PowerShell.

**Step 1: Launch the desktop app**

```powershell
cd "D:\Development\Projects\cpp\ECNU-VPN"
.\scripts\build-windows.ps1 debug-run
```

Or launch the packaged app from the output directory if `desktop:build` completed.

**Step 2: Install helper service through the UI**

- Navigate to the Service page in the UI.
- Click the install button.
- Wait for the status to update.

Verify:
- UI reports: installed = yes, running = yes, available = yes

**Step 3: Confirm CLI and desktop RPC agree**

```powershell
cd "D:\Development\Projects\cpp\ECNU-VPN"
.\build\windows\cpp\exv.exe service status
.\build\windows\cpp\exv.exe desktop-rpc service.status "{}"
```

Verify:
- Both report: Installed: yes, State: running, Socket Ready: yes
- No helper-unavailable error

**Step 4: Save auth settings**

- Navigate to the Settings/Auth page in the UI.
- Enter credentials and save.

Verify:
- Settings save without IPC clone error
- Settings persist after app restart

**Step 5: Connect through helper**

- Click Connect on the Dashboard.
- Wait for connection to establish.

Verify:
- Status changes to connected
- No helper-unavailable error
- Button state updates correctly (Connect becomes Disconnect)

**Step 6: Disconnect through helper**

- Click Disconnect.
- Wait for disconnection to complete.

Verify:
- Status returns to disconnected
- No stale VPN process remains
- Button state updates correctly (Disconnect becomes Connect)

**Step 7: Uninstall service through the UI**

- Navigate to the Service page.
- Click the uninstall button.
- Wait for SCM to settle (may take a few seconds).

Verify:
- UI reports: installed = no, running = no, available = no
- No false uninstall-incomplete state after SCM settles

---

### R1.3 Windows Package Runtime Inspection [develop-blocker]

Prerequisite: `npm run desktop:build` completed in R1.1.

```powershell
cd "D:\Development\Projects\cpp\ECNU-VPN"
$pkgExe = Get-ChildItem -Path "webui\release" -Recurse -Filter "exv.exe" | Select-Object -First 1
& $pkgExe.FullName desktop-rpc runtime.status "{}"
```

Verify:
- Output shows bundled OpenConnect and driver asset paths
- Paths point inside the package's `resources/bin` directory
- No dependency on `EXV_PATH` or `ECNUVPN_RUNTIME_DIR` environment variables

Check that `resources/bin` is not a stale copy:

```powershell
$resourcesBin = Get-ChildItem -Path "webui\release" -Recurse -Filter "resources" -Directory | Get-ChildItem -Filter "bin" -Directory | Select-Object -First 1
Get-ChildItem $resourcesBin.FullName | Select-Object Name, LastWriteTime
```

Verify: timestamps are recent (match the current build), not from an older release.

---

### R1.4 Windows Release-Only Fallback Matrix [release-blocker, NOT a develop-blocker]

Mark as: **Not tested for develop merge. Tested post-merge during release hardening.**

Scenarios deferred to R5 release readiness:
- Normal-user no-service one-time elevated connect
- UAC denial path
- Runtime missing path
- Driver missing path
- Portable vs installer first launch

---

## R2: macOS Manual Functional Validation

### R2.1 macOS Automated Recheck [develop-blocker]

Open Terminal and run:

```bash
cd /Users/tomli/Development/Projects/CPP/ECNU-VPN
git switch integration/platform-convergence-next
git status --short --branch
```

Confirm: branch is `integration/platform-convergence-next`, working tree clean.

```bash
./scripts/validate-merge-prep-macos.sh
```

Success indicators (in order):
- `[merge-prep] Build frontend assets for native embedding...` completes
- `[merge-prep] Configure and build native targets...` completes
- `[merge-prep] Run focused native regression tests...` — all 5 tests pass
- `[merge-prep] Compile Electron main/preload and native staging...` completes
- `[merge-prep] macOS validation complete.` printed at the end

If a step fails: record the exact error output. Do not proceed to R2.2 until R2.1 passes.

Next, build the desktop package:

```bash
cd /Users/tomli/Development/Projects/CPP/ECNU-VPN/webui
npm run desktop:build
```

Success: `electron-builder` produces a `.app` bundle without errors.

---

### R2.2 macOS Helper-Installed Path [develop-blocker]

Prerequisite: R2.1 passed.

**Step 1: Install launchd helper from UI**

- Launch the desktop app (dev mode or packaged `.app`).
- Navigate to the Service page.
- Click Install Helper.
- Authenticate when prompted.

Verify:
- UI reports: installed = yes, running = yes, available = yes

**Step 2: Confirm CLI status agrees**

```bash
cd /Users/tomli/Development/Projects/CPP/ECNU-VPN
./build/macos/cpp/exv service status
./build/macos/cpp/exv desktop-rpc service.status "{}"
```

Verify:
- Both report helper installed and running
- Socket is ready

**Step 3: Connect using real credentials**

- Navigate to the Dashboard.
- Enter VPN credentials if not already saved.
- Click Connect.

Verify:
- Connection establishes
- Tunnel/network route becomes active (check with `ifconfig` or `netstat -rn`)
- Status indicator shows connected

**Step 4: Disconnect**

- Click Disconnect.
- Wait for disconnection to complete.

Verify:
- No stale OpenConnect process: `pgrep -fl openconnect` returns nothing
- No stale campus route: `netstat -rn | grep <campus-net>` returns nothing
- UI status returns to disconnected

**Step 5: Verify config file ownership**

```bash
ls -la ~/Library/Application\ Support/ecnu-vpn/config.json
```

Verify: owner is your user account, not root.

---

### R2.3 macOS Helper-Missing One-Time Elevated Path [develop-blocker]

Prerequisite: R2.2 passed (helper is currently installed).

**Step 1: Uninstall helper**

- Navigate to the Service page in the UI.
- Click Uninstall Helper.
- Authenticate when prompted.

Verify:
- UI reports: installed = no, running = no, available = no

Or via CLI:

```bash
sudo ./build/macos/cpp/exv helper uninstall
```

**Step 2: One-time elevated connect — accept prompt**

- Ensure helper is NOT installed (verify on Service page).
- Click Connect on the Dashboard.
- An `osascript` administrator prompt should appear.
- Enter your password and click OK.

Verify:
- Connection succeeds with one-time elevation
- Status shows connected

**Step 3: Disconnect the elevated session**

- Click Disconnect.
- Authenticate if prompted again.

Verify:
- Disconnection succeeds
- No `Killed: 9` error in logs
- No stale OpenConnect process

**Step 4: One-time elevated connect — cancel prompt**

- Click Connect again.
- When the `osascript` administrator prompt appears, click Cancel.

Verify:
- Connection does NOT proceed
- UI reports a structured cancellation/error state (not a crash or hang)
- No `Killed: 9` runtime signing/quarantine failure in logs

---

### R2.4 macOS Package Runtime Check [release-blocker, NOT a develop-blocker]

Prerequisite: `npm run desktop:build` completed in R2.1.

Mark as: **Not a develop-blocker. Tested post-merge during release hardening.**

If you want to check now:

```bash
# Find the packaged .app
ls -la /Users/tomli/Development/Projects/CPP/ECNU-VPN/webui/release/mac-arm64/

# Verify bundled runtime
find /Users/tomli/Development/Projects/CPP/ECNU-VPN/webui/release -name "openconnect" -o -name "*.dylib" | head -20

# Check signing
codesign -v /Users/tomli/Development/Projects/CPP/ECNU-VPN/webui/release/mac-arm64/*.app 2>&1
```

Verify:
- OpenConnect binary and dylibs are inside the `.app` bundle
- No dependency on Homebrew paths (`/opt/homebrew` or `/usr/local`)
- Code signing is valid (or ad-hoc signed for dev builds)

---

## Results Recording

After completing tests, record results in the merge playbook. Copy the template below and fill in each cell.

### Template

```
### R1/R2 Validation Results (recorded YYYY-MM-DD)

Branch: integration/platform-convergence-next
Commit: <git rev-parse HEAD>

| Test | Gate | Result | Notes |
|------|------|--------|-------|
| R1.1 Windows Automated Validation | develop-blocker | PASS/FAIL | |
| R1.2 Windows GUI Service Path | develop-blocker | PASS/FAIL | |
| R1.3 Windows Package Runtime | develop-blocker | PASS/FAIL | |
| R1.4 Windows Fallback Matrix | release-blocker | DEFERRED | Post-merge |
| R2.1 macOS Automated Recheck | develop-blocker | PASS/FAIL | |
| R2.2 macOS Helper-Installed Path | develop-blocker | PASS/FAIL | |
| R2.3 macOS Helper-Missing Elevated Path | develop-blocker | PASS/FAIL | |
| R2.4 macOS Package Runtime | release-blocker | DEFERRED | Post-merge |

Develop merge decision: GO / NO-GO
Reason: <if NO-GO, list blocking failures>
```

### Where to record

Append the filled template to the "Pending before develop merge" section in:

```
docs/merge-playbooks/windows-macos-merge.md
```

Replace each "pending" item with the corresponding PASS/FAIL result. All develop-blocker tests must show PASS before proceeding to R4 (Develop Merge).
