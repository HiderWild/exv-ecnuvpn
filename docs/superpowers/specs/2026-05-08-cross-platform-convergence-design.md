# Cross-Platform Convergence Design

**Date:** 2026-05-08
**Status:** Approved
**Target:** macOS + Linux + Windows full platform support

## Gap Analysis

### Fatal (Windows cannot compile)

| Gap | Location | Description |
|-----|----------|-------------|
| `fork()` unavailable | helper.cpp, vpn.cpp | Windows has no fork(), needs CreateProcess |
| `AF_UNIX` socket unavailable | helper.cpp daemon_main | Entire daemon loop is POSIX code |
| `getpeereid`/`SO_PEERCRED` unavailable | helper.cpp | Unix socket credential verification |
| No `exv-helper.exe` build target | CMakeLists.txt | Windows Service needs separate binary |
| Bash shebang in tunnel script | tunnel.cpp | `#!/bin/bash` meaningless on Windows |

### Severe (Compiles but missing functionality)

| Gap | Location | Description |
|-----|----------|-------------|
| WebUI uses AF_UNIX to connect helper | webui.cpp | Needs Named Pipe alternative |
| Traffic stats are stub | utils.cpp | Returns false, needs GetIfEntry2 on Windows |
| Linux openconnect install uses Homebrew | vpn.cpp | `#else` branch incorrectly copies macOS logic |
| Hardcoded launchd paths in webui | webui.cpp:677 | `/api/service` endpoint has no Linux/Windows paths |

### Minor (Functional but imperfect)

| Gap | Location | Description |
|-----|----------|-------------|
| Compiler warnings | main.cpp, helper.cpp | freopen/write return values unchecked |
| Windows Named Pipe auth is TODO | helper.cpp:1526 | Needs SID verification |
| Dockerfile only aarch64 | Dockerfile | Missing x86_64 coverage |
| No Windows install script | scripts/ | Missing install-win.ps1 |

## Architecture: Hybrid Strategy

### Split Principle

- **Split into files**: Code that calls system APIs (IPC, Service, process management)
- **Keep #ifdef**: Code that only differs in constants/paths/commands

### File Layout

```
src/
├── helper.cpp              # Shared logic (command parsing, JSON handling)
├── helper_daemon_mac.cpp   # macOS: launchd + AF_UNIX + fork
├── helper_daemon_linux.cpp # Linux: systemd + AF_UNIX + fork
├── helper_daemon_win.cpp   # Windows: Win Service + Named Pipe + CreateProcess
├── helper_ipc.hpp          # IpcServer abstract interface
├── vpn.cpp                 # Shared logic + #ifdef paths/commands
├── webui.cpp               # Shared logic + #ifdef IPC connection method
├── utils.cpp               # Shared logic + #ifdef stats/paths
├── main.cpp                # Shared logic + #ifdef signal handling
└── config.hpp              # All #ifdef constants (unchanged)
```

### IpcServer Interface

```cpp
// helper_ipc.hpp
class IpcServer {
public:
    virtual ~IpcServer() = default;
    virtual bool start(const std::string& path) = 0;
    virtual bool accept_client() = 0;       // Wait for and accept a client connection
    virtual bool verify_client() = 0;       // Verify connected client credentials
    virtual std::string read_request() = 0; // Read request from current client
    virtual bool send_response(const std::string& resp) = 0; // Send response to current client
    virtual void close() = 0;
};

std::unique_ptr<IpcServer> create_ipc_server();  // Factory function
```

Note: The interface hides platform-specific handle types (fd on POSIX, HANDLE on Windows) behind methods. Each platform implementation manages its own internal handles.

### CMake Build

```cmake
if(APPLE)
    target_sources(exv-helper PRIVATE helper_daemon_mac.cpp)
elseif(UNIX)
    target_sources(exv-helper PRIVATE helper_daemon_linux.cpp)
elseif(WIN32)
    target_sources(exv-helper PRIVATE helper_daemon_win.cpp)
endif()
```

## Implementation Phases

### Phase 1: Linux Completion (1-2 days)

Goal: `exv` fully usable on Linux

- Complete Linux tunnel script generation (ip route / ip tun / resolvconf)
- Fix vpn.cpp Linux openconnect install logic (should not use Homebrew)
- Complete systemd service file generation and install/uninstall commands
- Complete Linux path resolution (XDG specification)
- Docker CI: x86_64 + aarch64 dual-architecture build test
- Fix compiler warnings (freopen/write return values)

### Phase 2: Windows Compilable (2-3 days)

Goal: `exv` and `exv-helper.exe` compile and run on Windows

- Create helper_daemon_win.cpp (Named Pipe + Windows Service)
- Implement IpcServer abstraction and factory function
- Windows tunnel script generation (PowerShell / netsh / route.exe)
- Replace fork() with CreateProcess()
- Replace AF_UNIX with Named Pipe connection
- Adapt webui.cpp IPC connection for Named Pipe
- Add Windows build targets to CMakeLists.txt

### Phase 3: Windows Functional (2-3 days)

Goal: `exv connect` end-to-end works on Windows

- Windows traffic statistics (GetIfEntry2 / GetIfTable2)
- Windows DNS management (netsh / Set-DnsClientServerAddress)
- Named Pipe client authentication (SID verification)
- Windows install script (PowerShell)
- Windows CI (GitHub Actions windows-latest)

### Phase 4: CI and Quality (1-2 days)

Goal: Automated verification across all three platforms

- GitHub Actions: macOS / Linux / Windows triple-platform CI
- Per-platform compile + `exv version` + `exv help` smoke tests
- Docker multi-architecture build (linux/amd64 + linux/arm64)
- Optional: code coverage or static analysis

## Success Criteria

1. `exv version` runs on macOS, Linux, Windows
2. `exv connect` establishes VPN on all three platforms
3. `exv-helper` runs as privileged daemon on all three platforms
4. CI passes on all three platforms
5. No platform-specific compilation errors
