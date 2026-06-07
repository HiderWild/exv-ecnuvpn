# Phase 7: macOS Real Platform Validation

> **Date**: 2026-06-03
> **Status**: Template -- requires real VPN server and macOS hardware
> **Scope**: Manual test matrix, real VPN flow verification, system side-effect validation

---

## 1. Test Matrix

Every combination in this matrix must be executed and results recorded before release.

### 1.1 Hardware Architecture

| # | Architecture | CI Runner Equivalent | Notes |
|---|---|---|---|
| A1 | Apple Silicon (ARM64) | `macos-latest` | Native arm64 build; verify no Rosetta translation |
| A2 | Intel (x86_64) | `macos-13` | Native x86_64 build |

### 1.2 macOS Version

| # | macOS Version | Minimum Supported | Notes |
|---|---|---|---|
| V1 | macOS 14 Sonoma | Yes | Current primary target |
| V2 | macOS 15 Sequoia | Yes | Latest shipping release |

### 1.3 Installation Type

| # | Type | Procedure | Expected Result |
|---|---|---|---|
| I1 | Fresh install (DMG) | Mount DMG, drag to Applications, run | App launches, helper not installed |
| I2 | Upgrade install (DMG) | Overwrite existing installation with new DMG | Config preserved, helper binary replaced, service re-bootstrapped |
| I3 | Helper installed | Run `sudo exv service install` | LaunchDaemon plist written to `/Library/LaunchDaemons/com.ecnu.exv.helper.plist`, helper binary at `/usr/local/bin/exv-helper`, launchd loaded |
| I4 | Transient / foreground helper | Run `exv-helper --foreground` in terminal | Helper accepts IPC on `/var/run/exv-helper.sock`, responds to `hello`, exits on SIGTERM |

### 1.4 Full Matrix

| Config | Arch | Install | Helper Mode | Priority |
|---|---|---|---|---|
| M1 | ARM64 | Fresh | Transient | P0 |
| M2 | ARM64 | Fresh | Installed | P0 |
| M3 | ARM64 | Upgrade | Installed | P1 |
| M4 | Intel | Fresh | Transient | P0 |
| M5 | Intel | Fresh | Installed | P0 |
| M6 | Intel | Upgrade | Installed | P1 |

---

## 2. Real VPN Flow Verification

All tests in this section require a real AnyConnect VPN gateway. Use `example.invalid` placeholders in all documentation. Do NOT commit real credentials.

> **Test server**: `https://vpn.example.invalid` (replace with actual gateway)
> **Test username**: `testuser@example.invalid`
> **Test password**: Use Keychain or `config.json`; never hardcode.

### 2.1 Connect Success

| Step | Action | Expected Result |
|---|---|---|
| 1 | Configure server: `exv config set server https://vpn.example.invalid` | Config saved |
| 2 | Configure username: `exv config set username testuser@example.invalid` | Config saved |
| 3 | Connect: `exv start` | VPN connected; exit code 0 |
| 4 | Verify status: `exv service status` | `running: yes` |
| 5 | Verify route: `netstat -rn \| grep utun` | utun interface present with expected IP |
| 6 | Verify DNS: `scutil --dns` | VPN DNS resolver listed |

**Log path**: `~/.ecnuvpn/logs/exv-*.log`

### 2.2 Disconnect Success

| Step | Action | Expected Result |
|---|---|---|
| 1 | While connected, run: `exv stop` | VPN disconnected; exit code 0 |
| 2 | Verify status: `exv service status` | `running: no` |
| 3 | Verify route: `netstat -rn \| grep utun` | utun interface removed |
| 4 | Verify DNS: `scutil --dns` | Original DNS resolver restored |

### 2.3 Auto-Reconnect (auto_reconnect=true)

| Step | Action | Expected Result |
|---|---|---|
| 1 | Set auto-reconnect: `exv config set auto_reconnect true` | Config saved |
| 2 | Connect: `exv start` | Connected |
| 3 | Simulate network disruption (suspend/resume or disconnect Ethernet) | After network restores, VPN reconnects automatically within 10 seconds |
| 4 | Verify: `exv service status` | `running: yes` after reconnection |

### 2.4 No Auto-Reconnect (auto_reconnect=false)

| Step | Action | Expected Result |
|---|---|---|
| 1 | Set no auto-reconnect: `exv config set auto_reconnect false` | Config saved |
| 2 | Connect: `exv start` | Connected |
| 3 | Simulate network disruption | VPN stays disconnected |
| 4 | Verify: `exv service status` | `running: no` after disruption |

### 2.5 User Disconnect -> No Reconnect

| Step | Action | Expected Result |
|---|---|---|
| 1 | Set auto-reconnect: `exv config set auto_reconnect true` | Config saved |
| 2 | Connect: `exv start` | Connected |
| 3 | Disconnect: `exv stop` | Disconnected; exit code 0 |
| 4 | Wait 15 seconds | VPN does NOT reconnect |
| 5 | Verify: `exv service status` | `running: no` |

### 2.6 Helper Lost Behavior

| Step | Action | Expected Result |
|---|---|---|
| 1 | Connect: `exv start` | Connected |
| 2 | Kill helper: `sudo kill $(pgrep exv-helper)` | Helper terminated |
| 3 | Wait 5 seconds | Session detected as lost |
| 4 | Verify: `exv service status` | `running: no` or error message indicating helper unavailable |
| 5 | Verify route cleanup: `netstat -rn \| grep utun` | utun interface removed (cleaned up by OS/helper shutdown) |

### 2.7 Core Crash Cleanup

| Step | Action | Expected Result |
|---|---|---|
| 1 | Connect: `exv start` | Connected |
| 2 | Kill supervisor: `sudo kill $(pgrep -f __vpn-supervisor)` | Supervisor terminated |
| 3 | Wait 10 seconds | Helper detects supervisor loss |
| 4 | Verify session state: `cat /var/run/exv-helper-session.json` | Session cleared or marked as stopped |
| 5 | Verify route cleanup: `netstat -rn \| grep utun` | utun interface removed |

### 2.8 Keychain Save/Load/Delete

| Step | Action | Expected Result |
|---|---|---|
| 1 | Save credentials via UI or CLI | Credentials encrypted and stored in Keychain |
| 2 | Verify: Open Keychain Access.app, search for `com.ecnu-vpn` | Entry visible under "Generic Password" |
| 3 | Load credentials (start new session) | Credentials loaded from Keychain without prompt |
| 4 | Delete credentials: `exv config clearAuth` | Entry removed from Keychain |
| 5 | Verify: Search Keychain again | Entry no longer present |

---

## 3. System Side Effects

### 3.1 utun Interface Creation

**Before connect**:
```bash
ifconfig | grep utun
# Expected: no utun interfaces (unless other VPNs are running)
```

**During connect**:
```bash
ifconfig | grep utun
# Expected: utun0 (or utunN) with IP address assigned
```

**After disconnect**:
```bash
ifconfig | grep utun
# Expected: no utun interfaces
```

### 3.2 Route Add/Cleanup

**Before connect**:
```bash
netstat -rn -f inet > /tmp/routes-before.txt
```

**During connect**:
```bash
netstat -rn -f inet > /tmp/routes-during.txt
diff /tmp/routes-before.txt /tmp/routes-during.txt
# Expected: new routes added for VPN subnets, default route unchanged
#           split-tunnel routes present if configured
```

**After disconnect**:
```bash
netstat -rn -f inet > /tmp/routes-after.txt
diff /tmp/routes-before.txt /tmp/routes-after.txt
# Expected: no differences (all VPN routes cleaned up)
```

### 3.3 DNS Set/Restore

**Before connect**:
```bash
scutil --dns > /tmp/dns-before.txt
```

**During connect**:
```bash
scutil --dns > /tmp/dns-during.txt
# Expected: VPN DNS resolver (e.g., 10.0.0.1) listed as resolver
```

**After disconnect**:
```bash
scutil --dns > /tmp/dns-after.txt
diff /tmp/dns-before.txt /tmp/dns-after.txt
# Expected: no differences (original DNS restored)
```

### 3.4 Split Tunnel Routes

If split tunnel is configured (e.g., `10.0.0.0/8` and `172.16.0.0/12`):
```bash
netstat -rn -f inet | grep -E '10\.|172\.16\.'
# Expected: routes via utun interface for specified subnets
```

After disconnect:
```bash
netstat -rn -f inet | grep -E '10\.|172\.16\.'
# Expected: no VPN-specific routes (or routes returned to original gateway)
```

### 3.5 Server Endpoint Route Protection

During VPN connection, the VPN server's own IP must remain routed via the physical interface (not through the tunnel):
```bash
# During connect, find server IP:
SERVER_IP=$(dig +short vpn.example.invalid | head -1)
route -n get $SERVER_IP | grep interface
# Expected: en0 (or physical interface), NOT utun
```

---

## 4. Test Environment

### 4.1 Prerequisites

| Requirement | Details |
|---|---|
| macOS hardware | Apple Silicon Mac (M1/M2/M3) AND Intel Mac (or CI runner) |
| macOS version | 14 Sonoma or 15 Sequoia |
| Network | Internet access to AnyConnect VPN gateway |
| Credentials | Test account with valid AnyConnect credentials |
| Build | `exv` and `exv-helper` binaries built via `scripts/build-macos.sh all` |
| DMG | Built via `scripts/build-macos.sh desktop` |
| Admin access | `sudo` for helper service install/uninstall |

### 4.2 Useful Commands

```bash
# Build
scripts/build-macos.sh all        # Build C++ + tests + electron
scripts/build-macos.sh cpp        # Build native only
scripts/build-macos.sh test       # Run native tests
scripts/build-macos.sh desktop    # Full build + DMG

# Service management
sudo exv service install          # Install helper as LaunchDaemon
sudo exv service uninstall        # Remove helper service
exv service status                # Check service status

# Debugging
exv-helper --foreground           # Run helper in foreground
log stream --predicate 'process == "exv-helper"'  # Stream helper logs
tail -f ~/.ecnuvpn/logs/exv-*.log                 # Tail application logs
```

### 4.3 Log Paths

| Log | Path | Contents |
|---|---|---|
| Application log | `~/.ecnuvpn/logs/exv-YYYY-MM-DD.log` | Connection events, errors |
| Helper log | `/var/log/exv-helper.log` (or syslog) | Helper daemon events |
| launchd log | `log show --predicate 'process == "com.ecnu.exv.helper"'` | launchd service events |
| Native session state | `/var/run/exv-helper-session.json` | Current session state |
| Config | `~/.ecnuvpn/config.json` | User configuration |

---

## 5. Known Issues

| Issue | Severity | Description | Workaround |
|---|---|---|---|
| Keychain prompt on first use | Low | macOS may show Keychain access prompt on first credential save | User approves; subsequent accesses are silent |
| utun number allocation | Low | utun interface number may vary based on other VPN software | Use interface name from `ifconfig` output, not hardcoded `utun0` |
| LaunchDaemon shell wrapper | Low | Helper plist uses `/bin/sh -c` wrapper | Standard launchd pattern; add `/bin/sh` to codesign if hardened |
| Foreground helper no auto-restart | Info | `--foreground` mode does not restart on crash | Expected behavior for development mode |
| DNS resolver order | Medium | VPN DNS may not take priority if system has multiple resolvers | Verify with `scutil --dns` that VPN resolver appears first |

---

## 6. Test Results Template

For each matrix configuration, record results in this format:

```
### Config M{N}: {Architecture} / {Install Type} / {Helper Mode}

**Date**: YYYY-MM-DD
**Tester**: Name
**macOS Version**: 14.x / 15.x
**Build**: commit hash

| Test | Result | Notes |
|---|---|---|
| 2.1 Connect Success | PASS/FAIL | |
| 2.2 Disconnect Success | PASS/FAIL | |
| 2.3 Auto-Reconnect | PASS/FAIL | |
| 2.4 No Auto-Reconnect | PASS/FAIL | |
| 2.5 User Disconnect | PASS/FAIL | |
| 2.6 Helper Lost | PASS/FAIL | |
| 2.7 Core Crash Cleanup | PASS/FAIL | |
| 2.8 Keychain Operations | PASS/FAIL | |
| 3.1 utun Creation | PASS/FAIL | |
| 3.2 Route Cleanup | PASS/FAIL | |
| 3.3 DNS Restore | PASS/FAIL | |
| 3.4 Split Tunnel | PASS/FAIL | |
| 3.5 Server Route Protection | PASS/FAIL | |

**Overall**: PASS / FAIL
**Blockers**: (list any P0 failures)
```
