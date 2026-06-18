# macOS Real VPN Connection -- Manual Checklist

> **Date**: _______________
> **Tester**: _______________
> **macOS Version**: _______________
> **Architecture**: ARM64 / Intel (circle one)
> **Build Commit**: _______________
> **VPN Server**: `https://_______________` (do NOT commit real server URL)

---

## Evidence Redaction

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

## Pre-Flight

### P1. Build Verification

- [ ] `exv --version` returns expected version string
  ```
  Output: _________________________________
  ```
- [ ] `file $(which exv)` shows correct architecture (arm64 / x86_64)
  ```
  Output: _________________________________
  ```
- [ ] `exv-helper --version` returns expected version string
  ```
  Output: _________________________________
  ```

### P1b. Native-Only Preconditions

```bash
pgrep -fl openconnect || true
pgrep -fl 'exv|vpn' || true
exv desktop-rpc runtime.status '{}'
```

- [ ] No OpenConnect process is running
- [ ] No retired supervisor process is running
- [ ] Runtime status reports `engine=native` and `source=native`
- [ ] P0: XML auth + CSTP CONNECT reaches success or structured auth/CSD/SAML error
- [ ] P1: DNS, routes, and liveness work on macOS
- [ ] P2: challenge/group/CSD/DTLS fallback/reconnect behavior verified or explicitly marked not exercised
- [ ] P3: native-only process/package evidence captured

### P2. Baseline System Snapshot

Capture system state BEFORE any VPN operations.

```bash
netstat -rn -f inet > /tmp/exv-netstat-before.txt
scutil --dns > /tmp/exv-dns-before.txt
ifconfig > /tmp/exv-ifconfig-before.txt
```

- [ ] Baseline route table saved: `/tmp/exv-netstat-before.txt`
  ```
  Line count: ________
  ```
- [ ] Baseline DNS config saved: `/tmp/exv-dns-before.txt`
  ```
  Line count: ________
  ```
- [ ] Baseline interface list saved: `/tmp/exv-ifconfig-before.txt`
  ```
  utun interfaces present before VPN: YES / NO
  ```

### P3. Helper Service State

- [ ] Helper service installed: `sudo exv service install`
- [ ] `exv service status` shows:
  ```
  Installed       : yes / no
  Socket Ready    : yes / no
  VPN Running     : yes / no
  ```
- [ ] LaunchDaemon plist exists: `ls -la /Library/LaunchDaemons/com.ecnu.exv.helper.plist`
  ```
  Permissions: ________
  ```

---

## Test 1: Connect Success

- [ ] Configure VPN server: `exv config set server https://_______________`
- [ ] Configure username: `exv config set username _______________`
- [ ] Run: `exv start`
  ```
  Exit code: ________
  Output: _________________________________
  ```
- [ ] Verify connection status: `exv service status`
  ```
  VPN Running: yes / no
  ```
- [ ] Verify utun interface created: `ifconfig | grep utun`
  ```
  Output: _________________________________
  ```
- [ ] Verify route added: `netstat -rn -f inet > /tmp/exv-netstat-connected.txt`
  ```
  Route differences from baseline:
  ```
  ```bash
  diff /tmp/exv-netstat-before.txt /tmp/exv-netstat-connected.txt
  ```
  ```
  Output: _________________________________
  ```
- [ ] Verify DNS changed: `scutil --dns > /tmp/exv-dns-connected.txt`
  ```bash
  diff /tmp/exv-dns-before.txt /tmp/exv-dns-connected.txt
  ```
  ```
  Output: _________________________________
  ```
- [ ] Verify VPN DNS resolver is present: `scutil --dns | grep -A2 "resolver"`
  ```
  Output: _________________________________
  ```
- [ ] Verify server endpoint route protection:
  ```bash
  SERVER_IP=$(dig +short _______________ | head -1)
  route -n get $SERVER_IP | grep interface
  ```
  ```
  Output: _________________________________
  Expected: en0 or en1 (physical interface, NOT utun)
  ```

**Result**: PASS / FAIL

---

## Test 2: Disconnect Success

- [ ] Run: `exv stop`
  ```
  Exit code: ________
  Output: _________________________________
  ```
- [ ] Verify status: `exv service status`
  ```
  VPN Running: yes / no
  ```
- [ ] Verify utun removed: `ifconfig | grep utun`
  ```
  Output: _________________________________
  Expected: (empty)
  ```
- [ ] Verify route cleanup:
  ```bash
  netstat -rn -f inet > /tmp/exv-netstat-after-disconnect.txt
  diff /tmp/exv-netstat-before.txt /tmp/exv-netstat-after-disconnect.txt
  ```
  ```
  Output: _________________________________
  Expected: no differences
  ```
- [ ] Verify DNS restored:
  ```bash
  scutil --dns > /tmp/exv-dns-after-disconnect.txt
  diff /tmp/exv-dns-before.txt /tmp/exv-dns-after-disconnect.txt
  ```
  ```
  Output: _________________________________
  Expected: no differences
  ```

**Result**: PASS / FAIL

---

## Test 3: Auto-Reconnect (auto_reconnect=true)

- [ ] Set auto-reconnect: `exv config set auto_reconnect true`
- [ ] Connect: `exv start`
  ```
  Exit code: ________
  ```
- [ ] Simulate network disruption:
  - [ ] Option A: Suspend and resume laptop
  - [ ] Option B: Toggle Wi-Fi off/on
  - [ ] Option C: `sudo ifconfig en0 down && sleep 5 && sudo ifconfig en0 up`
- [ ] Wait 15 seconds after network restores
- [ ] Verify reconnection: `exv service status`
  ```
  VPN Running: yes / no
  Reconnected within 15s: yes / no
  ```

**Result**: PASS / FAIL

---

## Test 4: No Auto-Reconnect (auto_reconnect=false)

- [ ] Set no auto-reconnect: `exv config set auto_reconnect false`
- [ ] Connect: `exv start`
  ```
  Exit code: ________
  ```
- [ ] Simulate network disruption (same method as Test 3)
- [ ] Wait 15 seconds after network restores
- [ ] Verify NO reconnection: `exv service status`
  ```
  VPN Running: yes / no
  Expected: no
  ```

**Result**: PASS / FAIL

---

## Test 5: User Disconnect Prevents Reconnect

- [ ] Set auto-reconnect: `exv config set auto_reconnect true`
- [ ] Connect: `exv start`
  ```
  Exit code: ________
  ```
- [ ] User-initiated disconnect: `exv stop`
- [ ] Wait 15 seconds
- [ ] Verify NO reconnection: `exv service status`
  ```
  VPN Running: yes / no
  Expected: no
  ```

**Result**: PASS / FAIL

---

## Test 6: Helper Lost Behavior

- [ ] Connect: `exv start`
- [ ] Kill helper: `sudo kill $(pgrep exv-helper)`
- [ ] Wait 5 seconds
- [ ] Verify session detected as lost: `exv service status`
  ```
  Output: _________________________________
  ```
- [ ] Verify utun removed: `ifconfig | grep utun`
  ```
  Output: _________________________________
  Expected: (empty)
  ```

**Result**: PASS / FAIL

---

## Test 7: Core Crash Cleanup

- [ ] Connect: `exv start`
- [ ] Kill the Core connection owner: `sudo pkill -f "exv.*core"`
- [ ] Wait 10 seconds
- [ ] Verify session state cleared: `cat /var/run/exv-helper-session.json 2>/dev/null || echo "File not found"`
  ```
  Output: _________________________________
  ```
- [ ] Verify utun removed: `ifconfig | grep utun`
  ```
  Output: _________________________________
  ```

**Result**: PASS / FAIL

---

## Test 8: Keychain Save/Load/Delete

- [ ] Save credentials via `exv config saveAuth` or UI
- [ ] Verify Keychain entry: Open Keychain Access.app, search for `com.ecnu-vpn`
  ```
  Entry found: yes / no
  ```
- [ ] Load credentials (start new session): `exv start`
  ```
  Credentials loaded without prompt: yes / no
  ```
- [ ] Delete credentials: `exv config clearAuth`
- [ ] Verify Keychain entry removed: Search Keychain Access.app
  ```
  Entry found: yes / no (expected: no)
  ```

**Result**: PASS / FAIL

---

## Post-Test Cleanup

- [ ] Disconnect VPN: `exv stop` (if connected)
- [ ] Restore baseline DNS: `scutil --dns` matches `/tmp/exv-dns-before.txt`
- [ ] Restore baseline routes: `netstat -rn -f inet` matches `/tmp/exv-netstat-before.txt`
- [ ] Clean up temp files: `rm /tmp/exv-netstat-*.txt /tmp/exv-dns-*.txt /tmp/exv-ifconfig-before.txt`

---

## Sign-Off

| Field | Value |
|---|---|
| Overall Result | PASS / FAIL |
| Tests Passed | ___ / 8 |
| Tests Failed | ___ / 8 |
| Blockers | (list any P0 failures) |
| Tester Signature | _________________________________ |
| Date | _________________________________ |
| Reviewer | _________________________________ |
| Reviewer Signature | _________________________________ |
| Review Date | _________________________________ |

### Notes

Only include redacted summaries below. Do not paste raw logs, packet captures,
screenshots, real gateway hostnames, usernames, tokens, cookies, challenge
responses, or packet payloads.

```
_________________________________________________
_________________________________________________
_________________________________________________
_________________________________________________
```
