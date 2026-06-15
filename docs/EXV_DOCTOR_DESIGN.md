# exv doctor — User Diagnostics Design

> **Status:** Design document (no implementation)
> **Last updated:** 2026-06-03
> **Purpose:** Define the `exv doctor` command for user-facing diagnostics and support bundle generation.

---

## 1. Overview

`exv doctor` is a diagnostic command that reports system state, helper status, credential store backend, route/DNS snapshot, engine path, and version information. It is designed for:

1. **Self-service troubleshooting** — users run `exv doctor` and see a health report
2. **Support escalation** — users generate a support bundle that developers can analyze
3. **Beta validation** — testers use it to verify correct installation and configuration

**Critical constraint:** `exv doctor` MUST NOT output secrets (passwords, tokens, cookies, session IDs). All secret values must be redacted or replaced with placeholders.

---

## 2. Command Interface

### Basic Usage

```bash
# Print human-readable diagnostic report to stdout
exv doctor

# Print machine-readable JSON report
exv doctor --json

# Generate support bundle (zip with logs + diagnostics, redacted)
exv doctor --support-bundle [output-path]

# Check specific subsystem
exv doctor --check helper
exv doctor --check credential-store
exv doctor --check routes
exv doctor --check version
```

### Output Modes

| Mode | Flag | Description |
|------|------|-------------|
| Human | (default) | Colorized terminal output with sections |
| JSON | `--json` | Machine-readable JSON for tooling |
| Support bundle | `--support-bundle` | Zip archive with logs, diagnostics, redacted config |
| Check | `--check <subsystem>` | Run only one diagnostic check |

---

## 3. Diagnostic Sections

### 3.1 Version Information

```
=== Version ===
  exv:          3.3.0
  exv-helper:   3.3.0
  Build:        Release
  Commit:       a1e34b7 (if embedded)
  Platform:     windows-x64 / macos-arm64 / linux-x64
  Electron:     39.2.4 (if desktop app running)
```

**Data source:** `ECNUVPN_VERSION` compile definition, `exv --version`, `exv-helper --version`.

### 3.2 Helper Service Status

```
=== Helper Service ===
  Status:       running / stopped / not-installed
  Mode:         resident / transient / unknown
  PID:          12345 (if running)
  Uptime:       2h 34m (if running)
  Last heartbeat: 5s ago (if running)
  IPC channel:  named-pipe / unix-socket
  Version:      3.3.0
  Protocol:     single helper protocol
```

**Platform-specific:**
- Windows: Check via `sc query exv-helper` or named pipe connection attempt
- macOS: Check via launchd `launchctl list | grep ecnu` or helper binary probe
- Linux: Check via systemd or process probe

### 3.3 Credential Store Backend

```
=== Credential Store ===
  Backend:      win-credential-store / darwin-keychain / linux-unsupported
  Status:       available / unavailable / unsupported
  Stored items: 2 (vpn-credentials, server-cert-hash)
```

**Security:** This section MUST NOT output the actual stored credentials. Only metadata (backend name, status, count of items) is shown.

**Data source:** `src/platform/common/credential_store.cpp` platform detection.

### 3.4 Route & DNS Snapshot

```
=== Network Routes ===
  Default gateway:   192.168.1.1
  VPN routes:        10.0.0.0/8 via utun0 (active) / wintun (active) / none
  Server route:      203.0.113.1 → direct (not tunneled)

=== DNS Configuration ===
  Primary DNS:       10.0.0.53 (via VPN)
  Fallback DNS:      8.8.8.8 (system default)
  DNS search:        ecnu.edu.cn
```

**Platform-specific:**
- Windows: `route print`, `netsh interface ip show dns`, Wintun adapter state
- macOS: `netstat -rn`, `scutil --dns`, utun state
- Linux: `ip route`, `resolvectl status`, tun state

**Security:** Internal IP addresses and DNS servers are safe to show. External server IPs should be shown as they are connection endpoints, not secrets.

### 3.5 Engine & Binary Path

```
=== Engine ===
  Binary path:  C:\Program Files\ECNU-VPN\bin\exv.exe
  Helper path:  C:\Program Files\ECNU-VPN\bin\exv-helper.exe
  Runtime dir:  C:\Program Files\ECNU-VPN\bin\
  Wintun DLL:   present (427 KB)
  OpenSSL:      system / bundled
```

**Security:** Paths may reveal installation location but are not secrets.

### 3.6 Tunnel State

```
=== Tunnel ===
  State:        disconnected / connecting / connected / reconnecting / failed
  Adapter:      utun0 / wintun / none
  IP assigned:  10.66.66.2 (if connected)
  Bytes in/out: 1.2 MB / 345 KB (if connected)
  Duration:     15m 23s (if connected)
```

### 3.7 Recent Errors

```
=== Recent Errors (last 5) ===
  [2026-06-03 14:23:01] WARN  helper heartbeat timeout (recovered)
  [2026-06-03 14:15:00] ERROR auth_failed: invalid credentials
  [2026-06-03 14:10:00] INFO  auto-reconnect triggered
```

**Security:** Error messages MUST NOT contain passwords, tokens, or cookies. The `no_secret_in_logs_test` validates this.

---

## 4. Support Bundle

### Purpose

A support bundle is a zip archive containing all diagnostic information needed by developers to diagnose user issues, with secrets redacted.

### Contents

```
exv-support-bundle-<timestamp>.zip
  doctor-report.json          # Full exv doctor --json output
  doctor-report.txt           # Human-readable report
  exv.log                     # Recent log file (redacted)
  exv-helper.log              # Helper log file (redacted)
  config.json                 # Current configuration (redacted)
  system-info.json            # OS, arch, installed software
  routes-before.txt           # Route table snapshot
  routes-after.txt            # Route table after connect/disconnect
  dns-before.txt              # DNS configuration snapshot
  dns-after.txt               # DNS configuration after connect/disconnect
```

### Redaction Rules

All files in the support bundle MUST be processed through the redaction layer before inclusion:

| Pattern | Replacement | Rationale |
|---------|-------------|-----------|
| Passwords | `[REDACTED-PASSWORD]` | Never expose credentials |
| Tokens | `[REDACTED-TOKEN]` | Session/auth tokens are secrets |
| Cookies | `[REDACTED-COOKIE]` | Session cookies are secrets |
| Session IDs | `sess-***` | Partial redaction for debugging |
| API keys | `[REDACTED-KEY]` | Third-party keys are secrets |
| Certificates | `[REDACTED-CERT]` | Private keys must not leak |

### Redaction Implementation

```cpp
// Pseudocode for redaction layer
std::string redact_secrets(const std::string& input) {
    std::string output = input;
    // Match common secret patterns
    output = regex_replace(output, R"(password[=:]\s*\S+)", "password=[REDACTED-PASSWORD]");
    output = regex_replace(output, R"(token[=:]\s*\S+)", "token=[REDACTED-TOKEN]");
    output = regex_replace(output, R"(cookie[=:]\s*\S+)", "cookie=[REDACTED-COOKIE]");
    output = regex_replace(output, R"(webvpn_session[=:]\s*\S+)", "webvpn_session=[REDACTED-TOKEN]");
    output = regex_replace(output, R"(auth_token[=:]\s*\S+)", "auth_token=[REDACTED-TOKEN]");
    return output;
}
```

### Generation

```bash
# Generate support bundle
exv doctor --support-bundle /tmp/exv-support.zip

# The bundle is automatically redacted
# Users can inspect before sending
exv doctor --support-bundle --dry-run  # Preview without creating zip
```

---

## 5. Log Layering

The diagnostic system uses four distinct log layers:

### Layer 1: User-Readable Errors

- **Audience:** End users
- **Destination:** Terminal stdout, UI error dialogs
- **Content:** Human-friendly error messages with suggested actions
- **Example:** "Connection failed. Check your username and password, then try again."
- **Redaction:** N/A (never contains secrets by design)

### Layer 2: Debug Logs

- **Audience:** Developers and advanced users
- **Destination:** Log files (`exv.log`, `exv-helper.log`)
- **Content:** Detailed protocol messages, state transitions, timing data
- **Redaction:** Applied before writing. Secrets replaced with `[REDACTED-*]`.
- **Level control:** `--verbose` flag or `ECNUVPN_LOG_LEVEL=debug` environment variable

### Layer 3: Support Bundle

- **Audience:** Development team
- **Destination:** Zip archive generated by `exv doctor --support-bundle`
- **Content:** All of the above plus system info, route/DNS snapshots, config dump
- **Redaction:** Mandatory. Every file processed through redaction layer.
- **Contains:** No secrets. Safe to email or attach to GitHub issue.

### Layer 4: Redaction Rules

- **Audience:** Redaction system itself
- **Destination:** Embedded in binary as compile-time regex patterns
- **Content:** Regex patterns for secret detection and replacement
- **Update mechanism:** New patterns added as new secret types are introduced
- **Validation:** `no_secret_in_logs_test` verifies redaction works

### Log File Locations

| Platform | Log Location |
|----------|-------------|
| Windows | `%APPDATA%\ECNU VPN\logs\exv.log` |
| macOS | `~/Library/Logs/ECNU VPN/exv.log` |
| Linux | `~/.local/share/ecnu-vpn/logs/exv.log` |

---

## 6. Implementation Plan

### Phase 1: Minimal Stub (Beta)

1. Add `doctor` subcommand to `exv` CLI parser in `src/main.cpp`
2. Implement version check (trivial — read compile-time version)
3. Implement helper status check (probe IPC channel or process)
4. Implement credential store backend detection
5. Output human-readable report to stdout
6. Add `--json` flag for machine output

### Phase 2: Full Diagnostics (Post-Beta)

1. Route and DNS snapshot (platform-specific)
2. Tunnel state reporting
3. Recent error log tail
4. Engine and binary path verification
5. `--check` subsystem filtering

### Phase 3: Support Bundle (Post-Beta)

1. Redaction layer implementation
2. Bundle generation with zip creation
3. `--dry-run` preview mode
4. Integration with log rotation

### Phase 4: Integration (Post-Beta)

1. Electron UI integration (run doctor from settings page)
2. Auto-diagnose on connection failure
3. Upload support bundle to feedback system
4. CI integration for smoke testing

---

## 7. Security Considerations

1. **Never output secrets** — The redaction layer is the last line of defense. Even if a new code path accidentally logs a secret, the redaction regex catches it before file inclusion.
2. **Validate with tests** — `no_secret_in_logs_test` must be extended to cover doctor output.
3. **Support bundle inspection** — Before sending a support bundle, users should review the contents. The `--dry-run` flag shows what would be included.
4. **No remote upload by default** — Support bundles are local files. Upload requires explicit user action.
5. **Redaction patterns are not secrets** — The regex patterns themselves are safe to expose (they match generic patterns, not specific values).
