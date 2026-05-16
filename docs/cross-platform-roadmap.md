# Cross-Platform Porting Roadmap: Linux + Windows

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Port ECNU-VPN from macOS-only to a cross-platform application supporting Linux and Windows, using `#ifdef` platform guards within existing files.

**Architecture:** Platform-specific code is isolated via `#ifdef __APPLE__` / `#elif defined(__linux__)` / `#elif defined(_WIN32)` within each source file. No new platform/ directory — the project is small enough for inline guards. Each platform gets its own service manager (launchd / systemd / Windows Service), IPC mechanism (Unix socket+getpeereid / Unix socket+SO_PEERCRED / Named Pipe+Impersonate), and crypto backend (CommonCrypto / OpenSSL / OpenSSL).

**Tech Stack:** C++17, CMake 3.15+, OpenSSL 1.1.1+ (Linux/Windows), cpp-httplib, nlohmann/json, Vue 3 WebUI

---

## Current State Summary

| File | Lines | macOS Guards | Linux Guards | Windows Guards | Status |
|------|-------|-------------|-------------|----------------|--------|
| CMakeLists.txt | ~80 | ✅ | ✅ | ❌ | Linux done |
| src/crypto.cpp | ~290 | ✅ | ✅ | ❌ | Linux done |
| src/utils.cpp | ~340 | ✅ | ✅ | ❌ | Linux done |
| src/sse_broadcaster.cpp | ~290 | ✅ | ✅ | ❌ | Linux done |
| src/tunnel.cpp | ~210 | ✅ | ✅ | ❌ | Linux done |
| src/helper.cpp | ~1350 | ✅ | ✅ | ❌ | Linux done |
| src/vpn.cpp | ~830 | ✅ | ✅ | ❌ | Linux done |
| src/config.hpp | ~50 | ✅ | ✅ | ❌ | Linux done |
| src/main.cpp | ~60 | ✅ | ✅ | ❌ | Linux done |
| src/logger.hpp | ~40 | N/A | N/A | N/A | Platform-independent |
| src/tunnel.hpp | ~30 | N/A | N/A | N/A | Platform-independent |
| Dockerfile | ~15 | N/A | ✅ | N/A | Created |

---

## Phase 1: Linux Completion & Verification

**Goal:** Get Linux to compile and run in Docker, then on real hardware.

### Task 1.1: Verify Docker Linux Build

**Files:**
- Existing: `Dockerfile`
- Existing: `CMakeLists.txt`

- [ ] **Step 1: Run Docker build**

```bash
docker build -t exv-linux .
```

Expected: Build succeeds with `0 errors, 0 warnings`.

- [ ] **Step 2: Verify binary runs in container**

```bash
docker run --rm exv-linux ./build/exv version
```

Expected: Prints version string like `exv x.y.z`.

- [ ] **Step 3: Verify config command works**

```bash
docker run --rm exv-linux ./build/exv config set server vpn-ct.ecnu.edu.cn
docker run --rm exv-linux ./build/exv config get server
```

Expected: Prints `vpn-ct.ecnu.edu.cn`.

**Acceptance Criteria:**
- `docker build` succeeds without errors
- `exv version`, `exv config get/set` work inside container
- macOS build (`cmake --build build`) still compiles and runs unchanged

---

### Task 1.2: Fix Docker Build Issues (if any)

**Files:**
- Modify: `CMakeLists.txt` (if linking errors)
- Modify: `Dockerfile` (if missing deps)
- Modify: `src/*.cpp` (if compilation errors)

This is a catch-all task for any issues discovered in Task 1.1. Common expected issues:

- Missing `#include <unistd.h>` on Linux (for `readlink`, `close`)
- Missing `#include <sys/un.h>` on Linux (for Unix socket)
- OpenSSL include path differences
- `kCCBlockSizeAES128` references not fully replaced

- [ ] **Step 1: Document each build error from Task 1.1**

- [ ] **Step 2: Fix each error with minimal `#ifdef` guards**

- [ ] **Step 3: Re-run Docker build to verify**

```bash
docker build -t exv-linux .
```

Expected: Clean build.

- [ ] **Step 4: Commit fixes**

```bash
git add -A
git commit -m "fix: resolve Linux compilation issues"
```

**Acceptance Criteria:**
- Docker build succeeds with zero errors
- All fixes are guarded by `#ifdef` — no macOS behavior changes

---

### Task 1.3: Add Linux Install Script

**Files:**
- Create: `scripts/install-linux.sh`

The existing `cminst.sh` is macOS-only (uses `brew`, macOS paths). Need a Linux equivalent.

- [ ] **Step 1: Create install script**

```bash
#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
BUILD_DIR="${SCRIPT_DIR}/../build"

if [ ! -f "${BUILD_DIR}/exv" ]; then
    echo "Build not found. Run: cmake -B build && cmake --build build"
    exit 1
fi

echo "Installing exv to /usr/local/bin..."
sudo cp "${BUILD_DIR}/exv" /usr/local/bin/exv
sudo cp "${BUILD_DIR}/exv-helper" /usr/local/bin/exv-helper
sudo chmod 4755 /usr/local/bin/exv-helper

echo "Installing openconnect (if not present)..."
if ! command -v openconnect &>/dev/null; then
    if command -v apt-get &>/dev/null; then
        sudo apt-get install -y openconnect
    elif command -v dnf &>/dev/null; then
        sudo dnf install -y openconnect
    elif command -v pacman &>/dev/null; then
        sudo pacman -S --noconfirm openconnect
    else
        echo "Please install openconnect manually."
    fi
fi

echo "Installation complete. Run 'exv' to start."
```

- [ ] **Step 2: Make executable and test**

```bash
chmod +x scripts/install-linux.sh
```

- [ ] **Step 3: Commit**

```bash
git add scripts/install-linux.sh
git commit -m "feat: add Linux install script"
```

**Acceptance Criteria:**
- Script installs exv + exv-helper to /usr/local/bin
- Script auto-installs openconnect via apt/dnf/pacman
- Script is idempotent (safe to run twice)

---

### Task 1.4: Real Hardware Integration Test

**Prerequisites:** Ubuntu 22.04 VM or bare metal with openconnect installed.

**Files:** None (testing only)

- [ ] **Step 1: Build on real Linux**

```bash
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(nproc)
```

Expected: Clean build.

- [ ] **Step 2: Test helper daemon lifecycle**

```bash
sudo ./build/exv install-helper
sudo systemctl status exv-helper
sudo ./build/exv uninstall-helper
```

Expected: Service installs, shows active, then uninstalls cleanly.

- [ ] **Step 3: Test VPN connection**

```bash
./build/exv connect
```

Expected: openconnect starts, TUN device created, routes added.

- [ ] **Step 4: Test split tunneling**

```bash
./build/exv config set split_tunnel true
./build/exv connect
ip route show
```

Expected: Only configured subnets route through TUN.

- [ ] **Step 5: Test WebUI**

```bash
./build/exv serve
# Open browser to http://localhost:18080
```

Expected: WebUI loads, shows connection status.

- [ ] **Step 6: Document any issues found**

**Acceptance Criteria:**
- `exv install-helper` / `uninstall-helper` work via systemd
- `exv connect` establishes VPN on Linux
- Split tunneling works with `ip route`
- WebUI serves and functions correctly

---

### Task 1.5: Add GitHub Actions CI for Linux

**Files:**
- Create: `.github/workflows/build.yml`

- [ ] **Step 1: Create CI workflow**

```yaml
name: Build
on: [push, pull_request]

jobs:
  build-macos:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - name: Build
        run: cmake -B build && cmake --build build -j$(sysctl -n hw.ncpu)
      - name: Test
        run: ./build/exv version

  build-linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install deps
        run: sudo apt-get install -y libssl-dev
      - name: Build
        run: cmake -B build && cmake --build build -j$(nproc)
      - name: Test
        run: ./build/exv version
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/build.yml
git commit -m "ci: add macOS + Linux build workflow"
```

**Acceptance Criteria:**
- CI runs on push/PR
- Both macOS and Linux builds pass
- `exv version` runs successfully on both platforms

---

## Phase 2: Windows Porting

**Goal:** Add Windows compilation and basic runtime support.

### Task 2.1: CMakeLists.txt — Windows Support

**Files:**
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Add Windows platform block**

After the existing Linux block, add:

```cmake
if(APPLE)
    # CommonCrypto links implicitly
elseif(UNIX AND NOT APPLE)
    find_package(OpenSSL 1.1.1 REQUIRED)
    target_link_libraries(exv PRIVATE OpenSSL::SSL OpenSSL::Crypto)
elseif(WIN32)
    find_package(OpenSSL 1.1.1 REQUIRED)
    target_link_libraries(exv PRIVATE OpenSSL::SSL OpenSSL::Crypto)
    # Windows Service API
    target_link_libraries(exv PRIVATE advapi32)
endif()
```

- [ ] **Step 2: Verify macOS build still works**

```bash
cmake --build build
```

Expected: No changes to macOS build.

- [ ] **Step 3: Commit**

```bash
git add CMakeLists.txt
git commit -m "feat: add Windows platform detection to CMake"
```

**Acceptance Criteria:**
- macOS build unchanged
- CMake configures correctly on Windows (when tested later)

---

### Task 2.2: src/crypto.cpp — Windows OpenSSL

**Files:**
- Modify: `src/crypto.cpp`

The Linux OpenSSL EVP code can be reused for Windows. Only the include guard needs extending.

- [ ] **Step 1: Extend platform guard**

Change:
```cpp
#elif defined(__linux__)
#include <openssl/evp.h>
#include <openssl/rand.h>
#endif
```

To:
```cpp
#elif defined(__linux__) || defined(_WIN32)
#include <openssl/evp.h>
#include <openssl/rand.h>
#endif
```

- [ ] **Step 2: Extend all `#elif defined(__linux__)` blocks in crypto.cpp**

Replace every occurrence of `#elif defined(__linux__)` with `#elif defined(__linux__) || defined(_WIN32)`.

The OpenSSL EVP code is identical on both platforms.

- [ ] **Step 3: Verify macOS build**

```bash
cmake --build build
```

- [ ] **Step 4: Commit**

```bash
git add src/crypto.cpp
git commit -m "feat: extend crypto OpenSSL EVP to Windows"
```

**Acceptance Criteria:**
- All `#elif defined(__linux__)` in crypto.cpp now include `_WIN32`
- macOS build unchanged
- OpenSSL EVP code covers both Linux and Windows

---

### Task 2.3: src/utils.cpp — Windows Paths and Commands

**Files:**
- Modify: `src/utils.cpp`

- [ ] **Step 1: Add Windows executable path**

In `get_executable_path()`, add after the Linux block:

```cpp
#elif defined(_WIN32)
    char buf[MAX_PATH];
    DWORD len = GetModuleFileNameA(NULL, buf, MAX_PATH);
    if (len == 0 || len == MAX_PATH) return "";
    return std::string(buf);
```

- [ ] **Step 2: Add Windows interface traffic**

In `get_interface_traffic()`, add after the Linux block:

```cpp
#elif defined(_WIN32)
    // Windows: use GetIfEntry2 from iphlpapi
    // For now, fall back to netsh command
    std::string output = run_command_output(
        "netsh interface ip show stats \"" + iface + "\" 2>nul");
    // Parse output for bytes received/sent
    // Simplified: return false until proper implementation
    return false;
```

- [ ] **Step 3: Add Windows openconnect paths**

In `get_openconnect_path()`, add:

```cpp
#elif defined(_WIN32)
    const char *candidates[] = {
        "C:\\Program Files\\OpenConnect\\openconnect.exe",
        "C:\\Program Files (x86)\\OpenConnect\\openconnect.exe",
        "openconnect.exe" // PATH fallback
    };
```

- [ ] **Step 4: Add Windows home directory**

Wherever `getpwuid()` or HOME is used, add:

```cpp
#elif defined(_WIN32)
    const char *home = std::getenv("APPDATA");
    if (!home) home = "C:\\Users\\Default\\AppData\\Roaming";
    return std::string(home) + "\\ecnuvpn";
```

- [ ] **Step 5: Verify macOS build**

```bash
cmake --build build
```

- [ ] **Step 6: Commit**

```bash
git add src/utils.cpp
git commit -m "feat: add Windows paths and commands to utils"
```

**Acceptance Criteria:**
- `get_executable_path()` works via `GetModuleFileNameA` on Windows
- `get_openconnect_path()` searches Windows install paths
- Config directory uses `%APPDATA%\ecnuvpn` on Windows
- macOS build unchanged

---

### Task 2.4: src/sse_broadcaster.cpp — Windows ReadDirectoryChangesW

**Files:**
- Modify: `src/sse_broadcaster.cpp`

- [ ] **Step 1: Add Windows include**

```cpp
#elif defined(_WIN32)
#include <windows.h>
#endif
```

- [ ] **Step 2: Add Windows file watcher implementation**

After the Linux inotify block, add a Windows implementation using `ReadDirectoryChangesW`:

```cpp
#elif defined(_WIN32)
    // Windows: ReadDirectoryChangesW
    HANDLE hDir = CreateFileA(
        std::filesystem::path(log_path_).parent_path().string().c_str(),
        FILE_LIST_DIRECTORY,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        NULL,
        OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
        NULL);

    if (hDir == INVALID_HANDLE_VALUE) {
        logger::error("SseBroadcaster: CreateFile failed for directory");
        return;
    }

    OVERLAPPED overlapped = {};
    overlapped.hEvent = CreateEvent(NULL, TRUE, FALSE, NULL);

    DWORD bytesReturned;
    alignas(DWORD) char notifyBuf[4096];
    std::string leftover;

    // Seek to end of existing file
    int fd = -1;
    if (utils::file_exists(log_path_)) {
        fd = ::open(log_path_.c_str(), O_RDONLY);
        if (fd >= 0) ::lseek(fd, 0, SEEK_END);
    }

    while (running_) {
        ZeroMemory(notifyBuf, sizeof(notifyBuf));
        ReadDirectoryChangesW(hDir, notifyBuf, sizeof(notifyBuf), FALSE,
                              FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME,
                              &bytesReturned, &overlapped, NULL);

        // Wait with 1-second timeout
        DWORD waitResult = WaitForSingleObject(overlapped.hEvent, 1000);
        if (waitResult == WAIT_TIMEOUT) {
            if (fd < 0 && utils::file_exists(log_path_)) {
                fd = ::open(log_path_.c_str(), O_RDONLY);
                if (fd >= 0) ::lseek(fd, 0, SEEK_END);
            }
            continue;
        }

        if (waitResult != WAIT_OBJECT_0) continue;

        // Parse notification
        FILE_NOTIFY_INFORMATION *pNotify = (FILE_NOTIFY_INFORMATION *)notifyBuf;
        bool modified = false;
        while (true) {
            if (pNotify->Action == FILE_ACTION_MODIFIED ||
                pNotify->Action == FILE_ACTION_RENAMED_NEW_NAME) {
                modified = true;
            }
            if (pNotify->NextEntryOffset == 0) break;
            pNotify = (FILE_NOTIFY_INFORMATION *)((char *)pNotify + pNotify->NextEntryOffset);
        }

        if (modified && fd >= 0) {
            char buf[4096];
            ssize_t nread;
            while ((nread = ::read(fd, buf, sizeof(buf))) > 0) {
                leftover.append(buf, static_cast<size_t>(nread));
                std::string::size_type pos;
                while ((pos = leftover.find('\n')) != std::string::npos) {
                    std::string line = leftover.substr(0, pos);
                    leftover.erase(0, pos + 1);
                    if (!line.empty()) {
                        parse_and_broadcast_log_line(line);
                    }
                }
            }
        }

        ResetEvent(overlapped.hEvent);
    }

    if (fd >= 0) close(fd);
    CloseHandle(overlapped.hEvent);
    CloseHandle(hDir);
#endif
```

- [ ] **Step 3: Verify macOS build**

```bash
cmake --build build
```

- [ ] **Step 4: Commit**

```bash
git add src/sse_broadcaster.cpp
git commit -m "feat: add Windows ReadDirectoryChangesW file watcher"
```

**Acceptance Criteria:**
- Windows file watcher uses `ReadDirectoryChangesW` with overlapped I/O
- File modification and rename events trigger log parsing
- macOS build unchanged

---

### Task 2.5: src/tunnel.cpp — Windows Route Commands

**Files:**
- Modify: `src/tunnel.cpp`

- [ ] **Step 1: Add Windows interface activation**

After the Linux `ip addr add` / `ip link set` block, add:

```cpp
#elif defined(_WIN32)
    // Windows: openconnect handles TUN setup via WinTUN/TAP driver
    // No manual ifconfig/ip needed — openconnect creates the interface
    ss << "echo \"Interface activation handled by openconnect\"\n";
```

- [ ] **Step 2: Add Windows route commands**

For each route block, add Windows `netsh` or `route add` commands:

```cpp
#elif defined(_WIN32)
    ss << "FOR /F \"tokens=4\" %%G IN ('route print 0.0.0.0 ^| findstr 0.0.0.0') DO set DEFAULT_GATEWAY=%%G\n";
```

For route exceptions:
```cpp
#elif defined(_WIN32)
    ss << "route delete " << server_ip << " >nul 2>&1\n";
    ss << "route add " << server_ip << " %DEFAULT_GATEWAY% >nul 2>&1\n";
```

For split tunnel routes:
```cpp
#elif defined(_WIN32)
    ss << "route delete " << route << " >nul 2>&1\n";
    ss << "route add " << route << " mask 255.255.255.255 %TUNDEV% >nul 2>&1\n";
```

- [ ] **Step 3: Verify macOS build**

```bash
cmake --build build
```

- [ ] **Step 4: Commit**

```bash
git add src/tunnel.cpp
git commit -m "feat: add Windows route commands to tunnel script"
```

**Acceptance Criteria:**
- Windows uses `route add/delete` commands
- openconnect handles TUN device creation on Windows
- macOS build unchanged

---

### Task 2.6: src/helper.cpp — Windows Service + Named Pipe

**Files:**
- Modify: `src/helper.cpp`

This is the most complex Windows port. The helper daemon becomes a Windows Service.

- [ ] **Step 1: Add Windows constants**

```cpp
#elif defined(_WIN32)
constexpr const char *kHelperServiceName = "exv-helper";
constexpr const char *kHelperPipePath = "\\\\.\\pipe\\exv-helper";
constexpr const char *kHelperStatePath = "C:\\ProgramData\\exv-helper-session.json";
```

- [ ] **Step 2: Add Windows service install**

```cpp
#elif defined(_WIN32)
    SC_HANDLE hSCM = OpenSCManager(NULL, NULL, SC_MANAGER_CREATE_SERVICE);
    if (!hSCM) {
        logger::error("Cannot open Service Control Manager");
        return false;
    }

    std::string exv_path = utils::get_executable_path();
    std::filesystem::path exv_dir = std::filesystem::path(exv_path).parent_path();
    std::string helper_bin = (exv_dir / "exv-helper.exe").string();

    SC_HANDLE hService = CreateServiceA(
        hSCM, kHelperServiceName, "ECNU VPN Helper",
        SERVICE_ALL_ACCESS, SERVICE_WIN32_OWN_PROCESS,
        SERVICE_AUTO_START, SERVICE_ERROR_NORMAL,
        helper_bin.c_str(), NULL, NULL, NULL, NULL, NULL);

    if (!hService) {
        DWORD err = GetLastError();
        if (err == ERROR_SERVICE_EXISTS) {
            std::cout << "Helper service is already installed.\n";
            CloseServiceHandle(hSCM);
            return true;
        }
        logger::error("CreateService failed: " + std::to_string(err));
        CloseServiceHandle(hSCM);
        return false;
    }

    StartService(hService, 0, NULL);
    CloseServiceHandle(hService);
    CloseServiceHandle(hSCM);

    std::cout << "Helper service installed and started.\n";
    return true;
```

- [ ] **Step 3: Add Windows service uninstall**

```cpp
#elif defined(_WIN32)
    SC_HANDLE hSCM = OpenSCManager(NULL, NULL, SC_MANAGER_CONNECT);
    if (!hSCM) return false;

    SC_HANDLE hService = OpenServiceA(hSCM, kHelperServiceName, SERVICE_STOP | DELETE);
    if (!hService) {
        std::cout << "Helper service is not installed.\n";
        CloseServiceHandle(hSCM);
        return true;
    }

    SERVICE_STATUS status;
    ControlService(hService, SERVICE_CONTROL_STOP, &status);
    DeleteService(hService);

    CloseServiceHandle(hService);
    CloseServiceHandle(hSCM);

    std::cout << "Helper service uninstalled.\n";
    return true;
```

- [ ] **Step 4: Add Windows service status**

```cpp
#elif defined(_WIN32)
    SC_HANDLE hSCM = OpenSCManager(NULL, NULL, SC_MANAGER_CONNECT);
    SC_HANDLE hService = OpenServiceA(hSCM, kHelperServiceName, SERVICE_QUERY_STATUS);
    bool installed = (hService != NULL);
    std::cout << "Helper service: " << (installed ? "installed" : "not installed") << "\n";
    if (installed) {
        SERVICE_STATUS status;
        QueryServiceStatus(hService, &status);
        std::cout << "  State: " << (status.dwCurrentState == SERVICE_RUNNING ? "running" : "stopped") << "\n";
        CloseServiceHandle(hService);
    }
    CloseServiceHandle(hSCM);
```

- [ ] **Step 5: Add Windows Named Pipe IPC**

Replace the Unix socket accept loop with Named Pipe on Windows:

```cpp
#elif defined(_WIN32)
    HANDLE hPipe = CreateNamedPipeA(
        kHelperPipePath,
        PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
        1, 4096, 4096, 0, NULL);

    if (hPipe == INVALID_HANDLE_VALUE) {
        logger::error("Helper: CreateNamedPipe failed");
        return;
    }

    while (running_) {
        OVERLAPPED ol = {};
        ol.hEvent = CreateEvent(NULL, TRUE, FALSE, NULL);
        ConnectNamedPipe(hPipe, &ol);

        DWORD waitResult = WaitForSingleObject(ol.hEvent, 1000);
        if (waitResult != WAIT_OBJECT_0) {
            CloseHandle(ol.hEvent);
            continue;
        }

        // Impersonate to get client identity
        if (!ImpersonateNamedPipeClient(hPipe)) {
            logger::error("Helper: ImpersonateNamedPipeClient failed");
            DisconnectNamedPipe(hPipe);
            CloseHandle(ol.hEvent);
            continue;
        }

        // Read request, process, write response (same JSON protocol)
        char buf[4096];
        DWORD bytesRead;
        if (ReadFile(hPipe, buf, sizeof(buf) - 1, &bytesRead, NULL) && bytesRead > 0) {
            buf[bytesRead] = '\0';
            std::string request(buf, bytesRead);
            std::string response = handle_request(request);
            DWORD bytesWritten;
            WriteFile(hPipe, response.c_str(), (DWORD)response.size(), &bytesWritten, NULL);
        }

        RevertToSelf();
        DisconnectNamedPipe(hPipe);
        CloseHandle(ol.hEvent);
    }
    CloseHandle(hPipe);
```

- [ ] **Step 6: Verify macOS build**

```bash
cmake --build build
```

- [ ] **Step 7: Commit**

```bash
git add src/helper.cpp
git commit -m "feat: add Windows Service + Named Pipe IPC to helper"
```

**Acceptance Criteria:**
- Windows helper uses Windows Service API (CreateService/DeleteService)
- IPC uses Named Pipe with ImpersonateNamedPipeClient for auth
- macOS build unchanged

---

### Task 2.7: src/vpn.cpp — Windows Network Commands

**Files:**
- Modify: `src/vpn.cpp`

- [ ] **Step 1: Add Windows tunnel interface detection**

```cpp
#elif defined(_WIN32)
    std::string ifconfig_out =
        utils::run_command_output("netsh interface show interface 2>nul | findstr /i \"openconnect\"");
```

- [ ] **Step 2: Add Windows openconnect install prompt**

```cpp
#elif defined(_WIN32)
    std::cout << "Install from: https://github.com/openconnect/openconnect-gui/releases\n";
```

- [ ] **Step 3: Verify macOS build**

```bash
cmake --build build
```

- [ ] **Step 4: Commit**

```bash
git add src/vpn.cpp
git commit -m "feat: add Windows network commands to vpn"
```

**Acceptance Criteria:**
- Windows uses `netsh` for interface detection
- Install prompt points to openconnect-gui releases
- macOS build unchanged

---

### Task 2.8: src/config.hpp + src/main.cpp — Windows Defaults

**Files:**
- Modify: `src/config.hpp`
- Modify: `src/main.cpp`

- [ ] **Step 1: Add Windows user-agent**

```cpp
#elif defined(_WIN32)
constexpr const char *DEFAULT_USER_AGENT =
    "AnyConnect Win_x86_64 4.10.06009";
```

And for the config struct:
```cpp
#elif defined(_WIN32)
    std::string useragent = "AnyConnect Win_x86_64 4.10.05095";
```

- [ ] **Step 2: Add Windows config directory**

```cpp
#elif defined(_WIN32)
    std::string config_dir = std::string(std::getenv("APPDATA")) + "\\ecnuvpn";
```

- [ ] **Step 3: Add Windows help text**

```cpp
#elif defined(_WIN32)
    std::cout << "  install-helper    Install Windows service (requires admin)\n";
    std::cout << "  uninstall-helper  Uninstall Windows service (requires admin)\n";
```

- [ ] **Step 4: Verify macOS build**

```bash
cmake --build build
```

- [ ] **Step 5: Commit**

```bash
git add src/config.hpp src/main.cpp
git commit -m "feat: add Windows defaults to config and help text"
```

**Acceptance Criteria:**
- User-Agent is `"AnyConnect Win_x86_64 ..."` on Windows
- Config directory is `%APPDATA%\ecnuvpn`
- Help text references "Windows service"
- macOS build unchanged

---

### Task 2.9: Windows Build Verification

**Files:** None (testing only)

- [ ] **Step 1: Build on Windows with MSVC**

```powershell
cmake -B build -G "Visual Studio 17 2022"
cmake --build build --config Release
```

Expected: Clean build.

- [ ] **Step 2: Test basic commands**

```powershell
.\build\Release\exv.exe version
.\build\Release\exv.exe config set server vpn-ct.ecnu.edu.cn
.\build\Release\exv.exe config get server
```

Expected: Version prints, config works.

- [ ] **Step 3: Test helper service**

```powershell
# Run as Administrator
.\build\Release\exv.exe install-helper
sc query exv-helper
.\build\Release\exv.exe uninstall-helper
```

Expected: Service installs, shows running, then uninstalls.

- [ ] **Step 4: Document issues found**

**Acceptance Criteria:**
- `exv.exe version` runs on Windows
- `exv.exe config get/set` works
- Helper service installs/uninstalls via Windows Service API

---

### Task 2.10: Add Windows CI

**Files:**
- Modify: `.github/workflows/build.yml`

- [ ] **Step 1: Add Windows job**

```yaml
  build-windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install OpenSSL
        run: choco install openssl -y
      - name: Build
        run: cmake -B build && cmake --build build --config Release
      - name: Test
        run: .\build\Release\exv.exe version
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/build.yml
git commit -m "ci: add Windows build to CI"
```

**Acceptance Criteria:**
- CI runs on all three platforms
- All three builds pass

---

## Phase 3: Polish & Release

### Task 3.1: Platform Documentation

**Files:**
- Modify: `README.md`
- Modify: `README_CN.md`

- [ ] **Step 1: Add Linux installation instructions**
- [ ] **Step 2: Add Windows installation instructions**
- [ ] **Step 3: Add platform-specific notes (TUN drivers, openconnect paths, etc.)**

**Acceptance Criteria:**
- README covers all three platforms
- Installation steps are copy-pasteable

---

### Task 3.2: Release Automation

**Files:**
- Create: `.github/workflows/release.yml`

- [ ] **Step 1: Create release workflow that builds on all three platforms**
- [ ] **Step 2: Upload artifacts: exv (macOS), exv (Linux), exv.exe (Windows)**

**Acceptance Criteria:**
- Tag push triggers release
- Binaries for all platforms are attached to GitHub Release

---

## Risk Register

| Risk | Impact | Mitigation |
|------|--------|------------|
| OpenSSL version mismatch on Linux | Build failure | CMake find_package with minimum 1.1.1 |
| openconnect not available on Windows | No VPN connection | Document openconnect-gui as prerequisite |
| Windows TUN driver missing | No tunnel | Document WinTUN/TAP installation |
| Named Pipe auth bypass | Security | Use ImpersonateNamedPipeClient + verify SID |
| `ReadDirectoryChangesW` misses events | Lost log lines | Overlapped I/O with proper event reset |
| MSVC vs GCC/Clang differences | Compilation errors | CI on all platforms catches early |

---

## Dependency Graph

```
Phase 1 (Linux):
  1.1 Docker Build ──► 1.2 Fix Issues ──► 1.3 Install Script
                                              │
                                              ▼
                                        1.4 Real HW Test
                                              │
                                              ▼
                                        1.5 CI

Phase 2 (Windows):
  2.1 CMake ──► 2.2 Crypto ──► 2.3 Utils ──► 2.4 SSE ──► 2.5 Tunnel ──► 2.6 Helper ──► 2.7 VPN ──► 2.8 Config ──► 2.9 Build Test ──► 2.10 CI

Phase 3 (Polish):
  3.1 Docs ──► 3.2 Release
```

Phase 1 and Phase 2 are **independent** — Windows work can start before Linux is fully verified, though it's recommended to complete Linux first to validate the `#ifdef` approach.
