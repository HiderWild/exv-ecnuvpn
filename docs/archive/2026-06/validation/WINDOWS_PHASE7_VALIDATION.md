# Windows Phase 7 — Real Platform Validation

> Version: 1.0
> Date: 2026-06-03
> Status: Ready for manual execution

---

## 1. Purpose

This document defines the **manual test matrix** for verifying ECNU-VPN on real
Windows hardware. Automated unit and integration tests cover logic; this matrix
covers the OS-level side effects that only real platform execution can confirm:
Wintun adapter lifecycle, route table mutations, DNS configuration, named pipe
IPC, helper service behavior, and crash/cleanup recovery.

---

## 2. Test Environment

### 2.1 Required Machines

| ID | OS | Account Type | Helper Mode | Notes |
|----|-----|-------------|-------------|-------|
| W11-A | Windows 11 23H2+ | Admin (elevated) | Resident (installed service) | Primary test host |
| W11-N | Windows 11 23H2+ | Normal user + UAC | Resident (installed service) | Verifies UAC elevation path |
| W10  | Windows 10 22H2+ | Admin or Normal | Either | Compatibility check (if available) |

### 2.2 Prerequisites

- Built binaries: `exv.exe`, `exv-helper.exe` (or NSIS-installed copy)
- Runtime DLLs staged: `runtime/win32-x64/wintun.dll`, MinGW runtime DLLs
  (`libgcc_s_seh-1.dll`, `libstdc++-6.dll`, `libwinpthread-1.dll`)
- Network access to VPN gateway (campus or test environment)
- Administrator credentials for service install/uninstall
- No existing ECNU-VPN process running before each test

### 2.3 Pre-Test Snapshot Commands

Run these before each test scenario to capture baseline state:

```powershell
# Route table snapshot
route print > "baseline-routes-$(Get-Date -Format 'yyyyMMdd-HHmmss').txt"

# DNS snapshot
ipconfig /all > "baseline-dns-$(Get-Date -Format 'yyyyMMdd-HHmmss').txt"

# Wintun adapter check
Get-NetAdapter -Name "ECNUTun" -ErrorAction SilentlyContinue

# Helper service status
sc.exe query exv-helper
```

### 2.4 Log Paths

| Log | Typical Location |
|-----|-----------------|
| exv core log | `%LOCALAPPDATA%\ecnu-vpn\logs\core.log` |
| exv helper log | `%LOCALAPPDATA%\ecnu-vpn\logs\helper.log` |
| Electron desktop log | `%LOCALAPPDATA%\ecnu-vpn\logs\desktop.log` |
| OpenConnect log (if applicable) | `%LOCALAPPDATA%\ecnu-vpn\logs\openconnect.log` |

> **Note:** Actual log paths may vary by build configuration. Check the
> `--verbose` output or application settings for the exact location on your
> test machine.

---

## 3. Manual Test Matrix

### 3.1 Platform / Account Matrix

Each row below is a distinct test run. Record PASS/FAIL for every check.

#### 3.1.1 Windows 11 Admin — Resident Helper

| # | Scenario | Steps | Expected Result | Pass/Fail |
|---|----------|-------|----------------|-----------|
| A-01 | Fresh install + connect | 1. Run NSIS installer as admin<br>2. Launch `exv` from install dir<br>3. Enter profile, click Connect | Wintun adapter created, IP assigned, routes added, DNS set, `Connected` phase | |
| A-02 | Disconnect clean | 1. While connected, click Disconnect | Routes removed, DNS restored, adapter removed (full policy), phase = `Idle` | |
| A-03 | Auto-reconnect: transport drop | 1. Connect with `auto_reconnect=true`<br>2. Kill `exv.exe` process (not helper) | Helper detects lease timeout (30s), cleans up. No residual routes/DNS. | |
| A-04 | Auto-reconnect: network disable | 1. Connect with `auto_reconnect=true`<br>2. Disable network adapter in Windows Settings<br>3. Re-enable adapter | Core enters `Reconnecting`, re-establishes tunnel, routes restored | |
| A-05 | Manual disconnect stops reconnect | 1. Connect with `auto_reconnect=true`<br>2. Click Disconnect | Phase = `Idle`, no reconnect attempts, no residual state | |
| A-06 | Service status check | 1. `sc.exe query exv-helper` | Service state = `RUNNING` | |
| A-07 | Helper hello / IPC | 1. Connect and check helper log for Hello handshake | Log shows `op: Hello`, `server_version: 2`, capabilities listed | |

#### 3.1.2 Windows 11 Normal User + UAC — Resident Helper

| # | Scenario | Steps | Expected Result | Pass/Fail |
|---|----------|-------|----------------|-----------|
| B-01 | Launch without admin | 1. Log in as normal user<br>2. Launch `exv.exe` | App starts, detects helper service is available (installed previously) | |
| B-02 | Connect as normal user | 1. Enter profile, click Connect | Helper service performs privileged ops. User does NOT get UAC prompt for connect. Wintun adapter created, IP/routes/DNS applied. | |
| B-03 | Disconnect as normal user | 1. Click Disconnect | Full cleanup, no UAC prompt | |
| B-04 | Service not installed | 1. Uninstall helper service<br>2. Launch `exv.exe` as normal user<br>3. Try to Connect | App shows error: helper unavailable. No UAC prompt. User must install helper from Settings or run installer as admin. | |

#### 3.1.3 Windows 10 (Compatibility)

| # | Scenario | Steps | Expected Result | Pass/Fail |
|---|----------|-------|----------------|-----------|
| C-01 | Basic connect/disconnect | Same as A-01 / A-02 | Same results on Win10 | |
| C-02 | Wintun adapter compatibility | Connect, check Device Manager | Wintun adapter visible under Network adapters | |
| C-03 | Route table format | `route print` during connection | Routes match expected VPN subnets | |

#### 3.1.4 Transient Helper (One-Time Elevation)

| # | Scenario | Steps | Expected Result | Pass/Fail |
|---|----------|-------|----------------|-----------|
| D-01 | Transient helper starts on connect | 1. No installed service<br>2. Launch `exv.exe` as admin (elevation)<br>3. Connect | `exv-helper.exe` process spawns, connects via named pipe, Hello returns `mode: "transient"` | |
| D-02 | Transient helper exits after idle | 1. Connect, then disconnect<br>2. Wait 60+ seconds<br>3. Check `Get-Process exv-helper` | Helper process exits after idle timeout (60s) | |
| D-03 | Transient helper during reconnect | 1. Connect (transient)<br>2. Disable network adapter<br>3. Re-enable | Helper stays alive during Reconnecting (heartbeats continue). After reconnect or give-up, cleanup runs. | |

#### 3.1.5 Portable Mode

| # | Scenario | Steps | Expected Result | Pass/Fail |
|---|----------|-------|----------------|-----------|
| E-01 | Portable connect/disconnect | 1. Extract portable build to USB or temp dir<br>2. Run `exv.exe` from extracted dir<br>3. Connect, then disconnect | VPN works. All DLLs found in same directory. No install needed. | |
| E-02 | Portable with transient helper | Run portable build as admin, connect | Transient helper mode works, no service install | |
| E-03 | Portable DLL resolution | Run portable, check `exv --version` | All runtime DLLs resolve from portable directory | |

---

### 3.2 Real VPN Flow Verification

These tests exercise the full VPN connection lifecycle on real infrastructure.

| # | Scenario | Steps | Expected Result | Pass/Fail |
|---|----------|-------|----------------|-----------|
| F-01 | Connect success | 1. Provide valid VPN profile (server, credentials)<br>2. Click Connect<br>3. Wait for `Connected` phase | Auth succeeds, CSTP established, Wintun adapter configured, IP/routes/DNS applied | |
| F-02 | Disconnect success | 1. While connected, click Disconnect | Phase transitions: `Connected` -> `Disconnecting` -> `CleaningUp` -> `Idle`. All routes/DNS removed. | |
| F-03 | auto_reconnect=true: transport drop | 1. Connect with `auto_reconnect=true`<br>2. Kill network cable or disable Wi-Fi<br>3. Restore network | Core enters `Reconnecting`. Exponential backoff visible in logs. Reconnects successfully. Routes/DNS restored. | |
| F-04 | auto_reconnect=false: no reconnect | 1. Connect with `auto_reconnect=false`<br>2. Kill network cable or disable Wi-Fi<br>3. Restore network | Core enters `Failed` state. No reconnect attempts. User must manually reconnect. | |
| F-05 | User disconnect: no reconnect | 1. Connect with `auto_reconnect=true`<br>2. Click Disconnect | Phase = `Idle`. `desired_connected=false` overrides auto_reconnect. No retry. | |
| F-06 | Helper lost: clear state | 1. Connect<br>2. Kill `exv-helper.exe` process | Core detects helper loss (heartbeat timeout). If `auto_reconnect=true`, attempts to recover. If not possible, enters `Failed`. State is cleared. | |
| F-07 | Core crash: helper cleanup | 1. Connect<br>2. Kill `exv.exe` process (core only) | Helper detects lease timeout (30s transient / 60s resident). Runs cleanup: removes routes, DNS, adapter. Session removed from registry. | |
| F-08 | Resident helper timeout cleanup | 1. Connect via resident helper<br>2. Disconnect (clean)<br>3. Wait 5 minutes | Helper remains running (`sc.exe query exv-helper` shows RUNNING). No stale sessions. Ready for next connection. | |
| F-09 | Transient helper timeout cleanup | 1. Connect via transient helper<br>2. Disconnect (clean)<br>3. Wait 60+ seconds | Helper process exits. No orphan processes. | |

---

### 3.3 System Side Effects Verification

Run these checks **during** and **after** each VPN flow test to verify OS-level effects.

| # | Check | Command (PowerShell) | Expected During Connect | Expected After Disconnect |
|---|-------|---------------------|------------------------|--------------------------|
| G-01 | Wintun adapter created | `Get-NetAdapter -Name "ECNUTun"` | Adapter exists, status = Up | Adapter removed (full cleanup policy) |
| G-02 | IP address assigned | `Get-NetIPAddress -InterfaceAlias "ECNUTun"` | VPN IP (e.g., 10.x.x.x) assigned | IP removed |
| G-03 | Routes added | `route print` or `Get-NetRoute -InterfaceAlias "ECNUTun"` | VPN subnet routes present (e.g., 10.0.0.0/8, 172.16.0.0/12) | VPN routes removed |
| G-04 | DNS set | `Get-DnsClientServerAddress -InterfaceAlias "ECNUTun"` | VPN DNS servers listed | DNS servers removed or restored to pre-VPN values |
| G-05 | Server endpoint NOT routed through tunnel | `tracert <vpn-server-ip>` | Traffic to VPN gateway goes via physical adapter, NOT through Wintun | N/A |
| G-06 | No residual routes after failure | After forced crash: `route print` | N/A | No VPN-related routes remain |
| G-07 | No residual DNS after failure | After forced crash: `ipconfig /all` | N/A | No VPN DNS servers remain |
| G-08 | No residual adapter after failure | After forced crash: `Get-NetAdapter -Name "ECNUTun"` | N/A | No ECNUTun adapter |

#### Verification Commands — Copy-Paste Ready

```powershell
# ---- During VPN connection (run in separate PowerShell window) ----

# Check Wintun adapter
Get-NetAdapter -Name "ECNUTun" -ErrorAction SilentlyContinue | Format-Table Name, Status, LinkSpeed

# Check assigned IP
Get-NetIPAddress -InterfaceAlias "ECNUTun" -ErrorAction SilentlyContinue | Format-Table IPAddress, PrefixLength

# Check routes on tunnel interface
Get-NetRoute -InterfaceAlias "ECNUTun" -ErrorAction SilentlyContinue | Format-Table DestinationPrefix, NextHop, RouteMetric

# Check DNS
Get-DnsClientServerAddress -InterfaceAlias "ECNUTun" -ErrorAction SilentlyContinue | Format-Table ServerAddresses

# Check VPN server is NOT routed through tunnel
# Replace <VPN_SERVER_IP> with actual gateway IP from profile
tracert -d <VPN_SERVER_IP> | Select-Object -First 10

# ---- After disconnect / cleanup (verify clean state) ----

# Wintun should be gone
Get-NetAdapter -Name "ECNUTun" -ErrorAction SilentlyContinue

# No VPN routes should remain
Get-NetRoute | Where-Object { $_.DestinationPrefix -match "^(10\.|172\.(1[6-9]|2[0-9]|3[01])\.|192\.168\.)" } | Format-Table DestinationPrefix, NextHop, InterfaceAlias

# No VPN DNS should remain
Get-DnsClientServerAddress | Where-Object { $_.InterfaceAlias -eq "ECNUTun" }
```

---

## 4. Known Issues

| ID | Description | Workaround | Status |
|----|-------------|------------|--------|
| KI-01 | Wintun adapter may not be removed if helper crashes during cleanup | Run `exv-helper.exe cleanup` manually or reboot | Open |
| KI-02 | Route cleanup may leave stale gateway routes on Wi-Fi disconnect | Manual `route delete <subnet>` | Open |
| KI-03 | DNS restoration may not apply if multiple VPN sessions overlapped | Restart DNS client service: `Restart-Service Dnscache` | Open |
| KI-04 | Transient helper UAC prompt may flash on slow machines | Use resident mode instead | By design |
| KI-05 | Portable mode does not install helper service | Use transient mode (run as admin) or install service separately | By design |

---

## 5. Test Execution Log Template

For each test run, record:

```
Date:       YYYY-MM-DD
Tester:     <name>
Machine ID: <W11-A / W11-N / W10>
OS Build:   <build number>
exv Build:  <commit hash or version>
Helper Mode: <resident / transient>

Test ID:    <A-01, B-02, etc.>
Result:     PASS / FAIL / BLOCKED
Notes:      <free text>

Evidence:
  - Screenshot: <path>
  - Log excerpt: <path>
  - Route dump: <path>
```

---

## 6. Sign-Off

| Area | Tester | Date | Status |
|------|--------|------|--------|
| Windows 11 Admin + Resident | | | |
| Windows 11 Normal + UAC | | | |
| Windows 10 Compatibility | | | |
| Transient Helper Mode | | | |
| Portable Mode | | | |
| VPN Flow (connect/disconnect) | | | |
| Auto-Reconnect Behavior | | | |
| Crash Recovery / Cleanup | | | |
| System Side Effects | | | |

---

## 7. Security Notes

- **Never** include real campus passwords, VPN tokens, SAML cookies, or session
  identifiers in test logs, screenshots, or this document.
- All test credentials in this document are placeholders. Replace with
  `user@example.invalid` / `password-placeholder` in any scripts.
- Do not commit screenshots that show real authentication flows to version control.
