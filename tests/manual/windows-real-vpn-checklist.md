# Windows Real VPN Testing Checklist

> Version: 1.0
> Date: 2026-06-03

---

## Overview

Step-by-step manual checklist for verifying ECNU-VPN on real Windows hardware
with a live VPN gateway. Each step has expected output and a sign-off field.

**Do not commit real credentials, tokens, cookies, raw logs, packet captures,
screenshots, or packet payloads.** All placeholder values use
`*.example.invalid`.

Before committing this checklist or any related evidence, confirm:

- [ ] Password values and real account or gateway identifiers are redacted.
- [ ] `webvpn=` values are redacted.
- [ ] `<session-token>` values are redacted.
- [ ] Opaque values are redacted.
- [ ] SAML values are redacted.
- [ ] Challenge responses are redacted.
- [ ] Cookies and cookie headers are redacted.
- [ ] Packet payloads are redacted.
- [ ] Command output, screenshots, logs, and captures are reduced to redacted summaries.

---

## Pre-Test Setup

### Step 0: Environment Record

| Field | Value |
|-------|-------|
| Tester name | |
| Date | |
| Windows version | (e.g., Windows 11 23H2 build 22631.xxx) |
| Machine ID | (e.g., W11-A, W11-N) |
| exv build commit | |
| Helper mode | resident / transient |
| VPN server | `vpn.example.invalid` (replace with actual server hostname) |

### Step 1: Clean State Verification

Run these commands and confirm output matches expected state.

**1a. Check no exv processes running:**

```powershell
Get-Process exv -ErrorAction SilentlyContinue
Get-Process exv-helper -ErrorAction SilentlyContinue
```

**Expected:** No output (no processes).

- [ ] No exv processes running

**1b. Check no Wintun adapter present:**

```powershell
Get-NetAdapter -Name "ECNUTun" -ErrorAction SilentlyContinue
```

**Expected:** No output or error "Cannot find adapter."

- [ ] No ECNUTun adapter

**1c. Capture baseline route table:**

```powershell
route print > "before-test-routes.txt"
```

**Expected:** File created. Note the number of routes for later comparison.

- [ ] Baseline route snapshot saved

**1d. Capture baseline DNS:**

```powershell
ipconfig /all > "before-test-dns.txt"
```

**Expected:** File created.

- [ ] Baseline DNS snapshot saved

**1e. Native-only preconditions:**

```powershell
Get-Process openconnect -ErrorAction SilentlyContinue
Get-Process | Where-Object { $_.ProcessName -match 'exv|vpn' }
.\exv.exe desktop-rpc runtime.status '{}'
```

**Expected:** No retired supervisor process. Runtime status reports `engine=native` and `source=native`.

- [ ] No OpenConnect process running
- [ ] No retired supervisor process running
- [ ] Runtime status is native-only

**1f. Native-only phase gates to record during this run:**

- [ ] P0: XML auth + CSTP CONNECT reaches success or structured auth/CSD/SAML error
- [ ] P1: DNS, routes, and liveness work on Windows
- [ ] P2: challenge/group/CSD/DTLS fallback/reconnect behavior verified or explicitly marked not exercised
- [ ] P3: native-only process/package evidence captured

---

## Connection Test

### Step 2: Connect to VPN

**2a. Launch exv and connect:**

```powershell
cd <build-or-install-dir>
.\exv.exe connect --profile default
```

Or use the desktop UI to enter profile details and click Connect.

**Expected output (CLI):**

```
Connecting to vpn.example.invalid...
Authenticating...
CSTP connected.
Applying network config...
Connected.
```

- [ ] Connection succeeds, phase = Connected

**2b. Verify Wintun adapter created:**

```powershell
Get-NetAdapter -Name "ECNUTun" | Format-Table Name, Status, LinkSpeed
```

**Expected:**

| Name | Status | LinkSpeed |
|------|--------|-----------|
| ECNUTun | Up | (varies) |

- [ ] ECNUTun adapter exists and is Up

**2c. Verify IP address assigned:**

```powershell
Get-NetIPAddress -InterfaceAlias "ECNUTun" | Format-Table IPAddress, PrefixLength
```

**Expected:** VPN IP assigned (e.g., `10.x.x.x` with prefix length like `/32` or `/24`).

- [ ] VPN IP address assigned

**2d. Verify routes added:**

```powershell
Get-NetRoute -InterfaceAlias "ECNUTun" | Format-Table DestinationPrefix, NextHop, RouteMetric
```

**Expected:** VPN subnet routes present. At minimum, the VPN gateway route.

- [ ] VPN routes present in route table

**2e. Verify DNS set:**

```powershell
Get-DnsClientServerAddress -InterfaceAlias "ECNUTun" | Format-Table ServerAddresses
```

**Expected:** VPN DNS server addresses listed.

- [ ] DNS servers configured on tunnel interface

**2f. Verify VPN server is NOT routed through tunnel:**

Replace `<VPN_SERVER_IP>` with the actual IP of your VPN gateway.

```powershell
tracert -d <VPN_SERVER_IP> | Select-Object -First 8
```

**Expected:** First hop is your default gateway (physical network), not the VPN tunnel.

- [ ] VPN gateway traffic does NOT go through tunnel

**2g. Test connectivity through tunnel:**

```powershell
ping <internal-resource-ip>
# or
curl -v https://portal.example.invalid
```

**Expected:** Successful ping/response through VPN tunnel.

- [ ] Connectivity through VPN tunnel works

---

## Disconnect Test

### Step 3: Disconnect from VPN

**3a. Disconnect:**

```powershell
.\exv.exe disconnect
```

Or use the desktop UI to click Disconnect.

**Expected:**

```
Disconnecting...
Cleaning up...
Disconnected.
```

- [ ] Disconnect completes, phase = Idle

**3b. Verify Wintun adapter removed:**

```powershell
Get-NetAdapter -Name "ECNUTun" -ErrorAction SilentlyContinue
```

**Expected:** No output (adapter removed).

- [ ] ECNUTun adapter removed

**3c. Verify routes cleaned up:**

```powershell
Get-NetRoute | Where-Object { $_.InterfaceAlias -eq "ECNUTun" }
```

**Expected:** No output (no routes on tunnel interface).

- [ ] VPN routes removed

**3d. Verify DNS cleaned up:**

```powershell
Get-DnsClientServerAddress -InterfaceAlias "ECNUTun" -ErrorAction SilentlyContinue
```

**Expected:** No output (no DNS on tunnel interface).

- [ ] VPN DNS removed

**3e. Compare route table to baseline:**

```powershell
route print > "after-disconnect-routes.txt"
diff before-test-routes.txt after-disconnect-routes.txt
```

**Expected:** No VPN-related routes differ from baseline.

- [ ] Route table matches pre-test baseline

---

## Auto-Reconnect Tests

### Step 4: Auto-Reconnect = true

**4a. Connect with auto_reconnect enabled:**

```powershell
.\exv.exe connect --profile default --auto-reconnect
```

- [ ] Connected successfully

**4b. Simulate transport drop (disable network):**

Open Windows Settings > Network & Internet. Toggle Wi-Fi off (or disable Ethernet adapter).

Wait 10 seconds, then re-enable.

**4c. Observe reconnection:**

```powershell
.\exv.exe status
```

**Expected:** Phase transitions from `Connected` -> `Reconnecting` -> `Connected`.
Logs show exponential backoff attempts.

- [ ] Auto-reconnect succeeds after transport drop

**4d. Verify routes/DNS restored:**

```powershell
Get-NetRoute -InterfaceAlias "ECNUTun" -ErrorAction SilentlyContinue
Get-DnsClientServerAddress -InterfaceAlias "ECNUTun" -ErrorAction SilentlyContinue
```

**Expected:** Routes and DNS are present after reconnection.

- [ ] Routes and DNS restored after reconnect

### Step 5: Auto-Reconnect = false

**5a. Connect with auto_reconnect disabled:**

```powershell
.\exv.exe connect --profile default --no-auto-reconnect
```

- [ ] Connected successfully

**5b. Simulate transport drop:**

Toggle Wi-Fi off, wait 10 seconds, re-enable.

**5c. Observe state:**

```powershell
.\exv.exe status
```

**Expected:** Phase = `Failed`. No automatic reconnect attempts. User must reconnect manually.

- [ ] No auto-reconnect when auto_reconnect=false

### Step 6: User Disconnect Stops Reconnect

**6a. Connect with auto_reconnect=true:**

```powershell
.\exv.exe connect --profile default --auto-reconnect
```

- [ ] Connected

**6b. Immediately disconnect:**

```powershell
.\exv.exe disconnect
```

**6c. Wait 30 seconds, check status:**

```powershell
Start-Sleep 30
.\exv.exe status
```

**Expected:** Phase = `Idle`. No reconnect attempts. `desired_connected=false` overrides auto_reconnect.

- [ ] User disconnect prevents auto-reconnect

---

## Crash Recovery Tests

### Step 7: Core Crash -> Helper Cleanup

**7a. Connect to VPN:**

```powershell
.\exv.exe connect --profile default
```

- [ ] Connected

**7b. Kill the core process:**

```powershell
Stop-Process -Name "exv" -Force
```

**7c. Wait for helper to detect lease timeout:**

For transient helpers: wait 30 seconds. For resident helpers: wait 60 seconds.

```powershell
Start-Sleep 35
```

**7d. Verify cleanup happened:**

```powershell
# Adapter should be gone
Get-NetAdapter -Name "ECNUTun" -ErrorAction SilentlyContinue

# Routes should be gone
Get-NetRoute | Where-Object { $_.InterfaceAlias -eq "ECNUTun" }

# DNS should be gone
Get-DnsClientServerAddress -InterfaceAlias "ECNUTun" -ErrorAction SilentlyContinue
```

**Expected:** All three show no output — helper cleaned up after core crash.

- [ ] Helper cleaned up after core crash

### Step 8: Transient Helper Timeout Exit

*Skip if using resident mode.*

**8a. Connect via transient helper, then disconnect:**

```powershell
.\exv.exe connect --profile default
.\exv.exe disconnect
```

**8b. Wait for idle timeout:**

```powershell
Start-Sleep 65
```

**8c. Check helper process:**

```powershell
Get-Process exv-helper -ErrorAction SilentlyContinue
```

**Expected:** No output — transient helper has exited.

- [ ] Transient helper exits after idle timeout

---

## Post-Test Cleanup

### Step 9: Final State Verification

**9a. No exv processes running:**

```powershell
Get-Process exv -ErrorAction SilentlyContinue
Get-Process exv-helper -ErrorAction SilentlyContinue
```

- [ ] No orphan processes

**9b. No ECNUTun adapter:**

```powershell
Get-NetAdapter -Name "ECNUTun" -ErrorAction SilentlyContinue
```

- [ ] No ECNUTun adapter

**9c. Route table clean:**

```powershell
route print > "final-routes.txt"
diff before-test-routes.txt final-routes.txt
```

- [ ] Route table matches pre-test baseline

**9d. DNS clean:**

```powershell
ipconfig /all > "final-dns.txt"
diff before-test-dns.txt final-dns.txt
```

- [ ] DNS matches pre-test baseline

---

## Native AnyConnect v2 Live Gates

Use this section only on a machine with valid ECNU credentials and administrator
rights. Attach the final evidence in a dated copy of
`docs/handoffs/native-anyconnect-v2-live-validation-template.md`; do not paste
raw logs, screenshots with secrets, packet captures, or session JSON into this
checklist.

### Redaction Checklist

Before committing evidence, confirm all of these have been removed or replaced:

- [ ] password values and password prompts with entered values
- [ ] `webvpn=` cookie values
- [ ] `<session-token>` values
- [ ] opaque values
- [ ] SAML values, assertions, redirects, and relay state
- [ ] challenge values and challenge responses
- [ ] cookies and cookie headers
- [ ] packet payloads and raw tunneled application data
- [ ] gateway hostnames, internal hostnames, and user identifiers unless approved

### Required Redacted Log Snippets

- [ ] Preflight substage timings
- [ ] Auth protocol stage
- [ ] CSTP CONNECT status
- [ ] Metadata summary
- [ ] Platform apply summary
- [ ] Liveness summary

### Phase P0: Auth and CSTP

| Check | Pass/Fail | Notes |
|-------|-----------|-------|
| Aggregate-auth init uses XML POST and AnyConnect headers | | |
| Username/password auth succeeds or returns a structured error | | |
| Group selection succeeds or returns `auth_group_required` | | |
| Challenge flow succeeds or returns `auth_challenge_required` | | |
| Successful auth maps the session token to a redacted `webvpn=` cookie | | |
| CSTP connects to `/CSCOSSLC/tunnel` with HTTP 200 | | |
| Failure is classified, not reported as a generic transport close | | |

### Phase P1: DNS, Routes, and Liveness

| Check | Pass/Fail | Notes |
|-------|-----------|-------|
| Metadata summary contains tunnel address, MTU, DNS, and route counts | | |
| ECNUTun address and MTU are applied | | |
| DNS servers and search domains are applied or explicitly unsupported | | |
| Split include and split exclude routes are applied deterministically | | |
| VPN gateway endpoint remains routed outside ECNUTun | | |
| Keepalive and DPD events appear when gateway timers are present | | |
| Disconnect restores baseline DNS and route state | | |

### Phase P2: DTLS, Challenge, CSD, and Reconnect

| Check | Pass/Fail | Notes |
|-------|-----------|-------|
| DTLS enabled path reports established, unavailable, or failed with fallback | | |
| CSTP-only fallback remains connected after DTLS is unavailable | | |
| Challenge or tokencode prompt is surfaced without leaking response values | | |
| CSD or host-scan requirement reports `csd_required_unsupported` | | |
| Reconnect first attempts cached cookie reuse when allowed | | |
| Expired cookie clears state and performs one full re-auth attempt | | |
| User disconnect prevents automatic reconnect | | |

---

## Sign-Off

Only enter redacted summaries in the Notes column. Do not paste raw logs,
packet captures, screenshots, real gateway hostnames, usernames, tokens,
cookies, challenge responses, or packet payloads.

| Step | Description | Pass/Fail | Notes |
|------|-------------|-----------|-------|
| 0 | Environment record | | |
| 1 | Clean state verification | | |
| 2 | Connect to VPN | | |
| 3 | Disconnect from VPN | | |
| 4 | Auto-reconnect (enabled) | | |
| 5 | Auto-reconnect (disabled) | | |
| 6 | User disconnect stops reconnect | | |
| 7 | Core crash -> helper cleanup | | |
| 8 | Transient helper timeout exit | | |
| 9 | Post-test cleanup verification | | |

**Tester signature:** ________________________  **Date:** ____________

**Reviewer signature:** ________________________  **Date:** ____________

---

## Troubleshooting

| Symptom | Likely Cause | Resolution |
|---------|-------------|------------|
| `exv-helper.exe` service timeout error 1053 | Service binary path incorrect or SCM handshake missing | Verify `exv-helper.exe` implements `StartServiceCtrlDispatcher`. Check `sc.exe qc exv-helper` for binary path. |
| Wintun adapter not created | `wintun.dll` not found or architecture mismatch | Verify `wintun.dll` is in the same directory as `exv.exe` or in the system PATH. Ensure 64-bit DLL for 64-bit process. |
| Routes not cleaned up | Helper crashed during cleanup | Run `route print` to identify stale routes. Manually delete with `route delete <subnet>`. |
| DNS not restored | Multiple VPN sessions overlapped | `Restart-Service Dnscache` to flush DNS client cache. |
| Helper "unavailable" on normal user account | Service not installed or not running | Run installer as admin, or `exv service install` from elevated prompt. |
| `STATUS_DLL_NOT_FOUND` on launch | Missing MinGW runtime DLLs | Ensure `libgcc_s_seh-1.dll`, `libstdc++-6.dll`, `libwinpthread-1.dll` are next to `exv.exe`. |
