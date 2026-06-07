# Cross-Platform Convergence Implementation Plan

> Status: CLOSED / SUPERSEDED.
> Closed on: 2026-05-22.
> Superseded by: `docs/superpowers/plans/2026-05-22-develop-merge-and-release-readiness.md`.
> Closure summary: the original cross-platform implementation plan is obsolete as the Windows/macOS platform convergence has landed in `integration/platform-convergence-next`. Linux completion, CI, and remaining broad cross-platform hardening are transferred to R5 as post-merge release/readiness work.
> Historical note: keep this file as the initial convergence blueprint only; do not use its unchecked boxes as the active task list.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make ECNU-VPN (exv) fully functional on macOS, Linux, and Windows with automated CI.

**Architecture:** Hybrid strategy — platform abstraction layer (PAL) for system APIs (IPC, Service, process management), inline `#ifdef` for constants/paths/commands. The `helper.cpp` daemon loop splits into per-platform files; other files keep `#ifdef` guards.

**Tech Stack:** C++17, CMake 3.15+, OpenSSL 1.1.1+ (Linux/Windows), CommonCrypto (macOS), cpp-httplib, nlohmann/json, Vue 3 WebUI

---

## File Structure

| File | Action | Responsibility |
|------|--------|----------------|
| `src/helper_ipc.hpp` | Create | IpcServer abstract interface + factory |
| `src/helper_daemon_mac.cpp` | Create | macOS daemon: launchd + AF_UNIX + getpeereid + fork |
| `src/helper_daemon_linux.cpp` | Create | Linux daemon: systemd + AF_UNIX + SO_PEERCRED + fork |
| `src/helper_daemon_win.cpp` | Create | Windows daemon: Win Service + Named Pipe + CreateProcess |
| `src/helper.cpp` | Modify | Remove daemon_main body, keep shared logic |
| `src/helper.hpp` | Modify | Add IpcServer forward declaration |
| `src/vpn.cpp` | Modify | Fix Linux openconnect install logic |
| `src/webui.cpp` | Modify | Platform-aware IPC connection + service path |
| `src/main.cpp` | Modify | Fix freopen warnings, Windows signal handling |
| `src/utils.cpp` | Modify | Windows traffic stats (GetIfEntry2) |
| `src/tunnel.cpp` | Modify | Windows PowerShell tunnel script |
| `CMakeLists.txt` | Modify | Platform-conditional daemon sources, exv-helper target |
| `Dockerfile` | Modify | Multi-arch support |
| `.github/workflows/build.yml` | Create | macOS + Linux + Windows CI |
| `scripts/install-win.ps1` | Create | Windows PowerShell installer |

---

## Phase 1: Linux Completion

### Task 1.1: Fix Linux openconnect install in vpn.cpp

**Files:**
- Modify: `src/vpn.cpp:353-390`

The `#else` branch (lines 353-390) incorrectly copies the macOS Homebrew install logic. On Linux, it should use `apt-get`/`dnf`/`pacman`.

- [ ] **Step 1: Replace the `#else` block in vpn.cpp openconnect install**

Find the `#else` block after `#elif defined(_WIN32)` (around line 353) that contains Homebrew logic. Replace it with Linux package manager detection:

```cpp
#else
    // Linux: try system package managers
    if (utils::run_command("which apt-get > /dev/null 2>&1") == 0) {
      utils::print_info("Running: sudo apt-get install -y openconnect ...");
      int ret = utils::run_command("sudo apt-get install -y openconnect");
      if (ret != 0 || !utils::check_openconnect()) {
        utils::print_error("apt-get install openconnect failed.");
        return 1;
      }
    } else if (utils::run_command("which dnf > /dev/null 2>&1") == 0) {
      utils::print_info("Running: sudo dnf install -y openconnect ...");
      int ret = utils::run_command("sudo dnf install -y openconnect");
      if (ret != 0 || !utils::check_openconnect()) {
        utils::print_error("dnf install openconnect failed.");
        return 1;
      }
    } else if (utils::run_command("which pacman > /dev/null 2>&1") == 0) {
      utils::print_info("Running: sudo pacman -S --noconfirm openconnect ...");
      int ret = utils::run_command("sudo pacman -S --noconfirm openconnect");
      if (ret != 0 || !utils::check_openconnect()) {
        utils::print_error("pacman install openconnect failed.");
        return 1;
      }
    } else {
      utils::print_error("No supported package manager found (apt-get/dnf/pacman).");
      utils::print_info("Install openconnect manually, then run exv again.");
      logger::error("openconnect not found, no package manager available");
      return 1;
    }
    utils::print_success("openconnect installed successfully!");
    logger::info("openconnect installed via system package manager");
```

- [ ] **Step 2: Verify macOS build unchanged**

Run: `cmake --build build -j$(sysctl -n hw.ncpu) 2>&1 | tail -5`
Expected: Build succeeds with no new errors.

- [ ] **Step 3: Verify Linux build in Docker**

Run: `docker build -t exv-linux-test . 2>&1 | tail -10`
Expected: Build succeeds.

- [ ] **Step 4: Commit**

```bash
git add src/vpn.cpp
git commit -m "fix: use system package managers for Linux openconnect install"
```

---

### Task 1.2: Fix compiler warnings (freopen/write return values)

**Files:**
- Modify: `src/main.cpp:515-517`
- Modify: `src/helper.cpp:951,1542`

- [ ] **Step 1: Fix freopen warnings in main.cpp**

Replace lines 515-517:
```cpp
        freopen("/dev/null", "r", stdin);
        freopen("/dev/null", "w", stdout);
        freopen("/dev/null", "w", stderr);
```
With:
```cpp
        (void)freopen("/dev/null", "r", stdin);
        (void)freopen("/dev/null", "w", stdout);
        (void)freopen("/dev/null", "w", stderr);
```

- [ ] **Step 2: Fix write warnings in helper.cpp**

At line 951, replace:
```cpp
  write(client_fd, payload.data(), payload.size());
```
With:
```cpp
  (void)write(client_fd, payload.data(), payload.size());
```

At line 1542, replace:
```cpp
      write(client_fd, payload.data(), payload.size());
```
With:
```cpp
      (void)write(client_fd, payload.data(), payload.size());
```

- [ ] **Step 3: Verify build**

Run: `cmake --build build -j$(sysctl -n hw.ncpu) 2>&1 | grep -i warning`
Expected: No warnings from main.cpp or helper.cpp.

- [ ] **Step 4: Commit**

```bash
git add src/main.cpp src/helper.cpp
git commit -m "fix: suppress freopen/write unused result warnings"
```

---

### Task 1.3: Fix webui.cpp hardcoded launchd path

**Files:**
- Modify: `src/webui.cpp:677`

- [ ] **Step 1: Replace hardcoded launchd path with platform-aware check**

Replace line 677:
```cpp
        j["installed"] = utils::file_exists("/Library/LaunchDaemons/com.ecnu.exv.helper.plist");
```
With:
```cpp
#ifdef __APPLE__
        j["installed"] = utils::file_exists("/Library/LaunchDaemons/com.ecnu.exv.helper.plist");
#elif defined(__linux__)
        j["installed"] = utils::file_exists("/etc/systemd/system/exv-helper.service");
#elif defined(_WIN32)
        j["installed"] = false; // TODO: check Windows SCM for service existence
#endif
```

- [ ] **Step 2: Verify build**

Run: `cmake --build build -j$(sysctl -n hw.ncpu) 2>&1 | tail -5`
Expected: Build succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/webui.cpp
git commit -m "fix: use platform-aware service install path in webui"
```

---

### Task 1.4: Update Dockerfile for multi-arch

**Files:**
- Modify: `Dockerfile`

- [ ] **Step 1: Update Dockerfile to support both amd64 and arm64**

Replace the entire Dockerfile:
```dockerfile
FROM --platform=$BUILDPLATFORM ubuntu:22.04

ARG TARGETPLATFORM

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential \
    cmake \
    libssl-dev \
    python3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

COPY . .

RUN cmake -B build -DCMAKE_BUILD_TYPE=Release && cmake --build build -j$(nproc)

# Verify the binary runs
RUN ./build/exv version

# Smoke test
RUN ./build/exv help
```

- [ ] **Step 2: Test build**

Run: `docker build -t exv-linux-test . 2>&1 | tail -10`
Expected: Build succeeds.

- [ ] **Step 3: Commit**

```bash
git add Dockerfile
git commit -m "feat: add multi-arch Dockerfile support"
```

---

## Phase 2: Windows Compilable — IpcServer Abstraction

### Task 2.1: Create IpcServer interface (helper_ipc.hpp)

**Files:**
- Create: `src/helper_ipc.hpp`

- [ ] **Step 1: Write the IpcServer abstract interface**

```cpp
#pragma once

#include <memory>
#include <string>

namespace ecnuvpn {
namespace helper {

class IpcServer {
public:
  virtual ~IpcServer() = default;

  // Start listening on the given path (socket path or pipe name)
  virtual bool start(const std::string &path) = 0;

  // Block until a client connects; returns true on success
  virtual bool accept_client() = 0;

  // Verify the connected client's credentials (uid/gid on POSIX, SID on Windows)
  // Returns true if the client is authorized
  virtual bool verify_client() = 0;

  // Read the full request from the current client (newline-delimited JSON)
  virtual std::string read_request() = 0;

  // Send a response to the current client
  virtual bool send_response(const std::string &response) = 0;

  // Close the server and release resources
  virtual void close() = 0;

  // Get the peer uid/gid after verify_client (POSIX only; Windows returns 0)
  virtual unsigned int peer_uid() const = 0;
  virtual unsigned int peer_gid() const = 0;
};

// Factory: creates the platform-appropriate IpcServer
std::unique_ptr<IpcServer> create_ipc_server();

} // namespace helper
} // namespace ecnuvpn
```

- [ ] **Step 2: Verify header compiles**

Add a temporary include in `src/helper.cpp`:
```cpp
#include "helper_ipc.hpp"
```
Run: `cmake --build build -j$(sysctl -n hw.ncpu) 2>&1 | tail -5`
Expected: Build succeeds (header is standalone).

- [ ] **Step 3: Commit**

```bash
git add src/helper_ipc.hpp
git commit -m "feat: add IpcServer abstract interface for platform IPC"
```

---

### Task 2.2: Create macOS IpcServer implementation

**Files:**
- Create: `src/helper_daemon_mac.cpp`

This extracts the macOS daemon loop from `helper.cpp:1455-1577` into the IpcServer interface.

- [ ] **Step 1: Write helper_daemon_mac.cpp**

```cpp
#include "helper_ipc.hpp"
#include "logger.hpp"

#include <cerrno>
#include <cstring>
#include <cstdio>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace ecnuvpn {
namespace helper {

class MacIpcServer : public IpcServer {
  int server_fd_ = -1;
  int client_fd_ = -1;
  unsigned int peer_uid_ = 0;
  unsigned int peer_gid_ = 0;

public:
  ~MacIpcServer() override { close(); }

  bool start(const std::string &path) override {
    server_fd_ = socket(AF_UNIX, SOCK_STREAM, 0);
    if (server_fd_ < 0)
      return false;

    sockaddr_un addr {};
    addr.sun_family = AF_UNIX;
    std::snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", path.c_str());

    if (bind(server_fd_, reinterpret_cast<sockaddr *>(&addr), sizeof(addr)) != 0) {
      ::close(server_fd_);
      server_fd_ = -1;
      return false;
    }

    // macOS: group staff (gid 20) — all local user accounts
    chmod(path.c_str(), 0660);
    chown(path.c_str(), 0, 20);

    if (listen(server_fd_, 8) != 0) {
      ::close(server_fd_);
      server_fd_ = -1;
      std::remove(path.c_str());
      return false;
    }
    return true;
  }

  bool accept_client() override {
    client_fd_ = ::accept(server_fd_, nullptr, nullptr);
    if (client_fd_ < 0)
      return false;

    // Set close-on-exec
    int flags = fcntl(client_fd_, F_GETFD);
    if (flags >= 0)
      fcntl(client_fd_, F_SETFD, flags | FD_CLOEXEC);

    return true;
  }

  bool verify_client() override {
    uid_t uid = 0;
    gid_t gid = 0;
    if (getpeereid(client_fd_, &uid, &gid) != 0) {
      ::close(client_fd_);
      client_fd_ = -1;
      return false;
    }
    peer_uid_ = static_cast<unsigned int>(uid);
    peer_gid_ = static_cast<unsigned int>(gid);
    return true;
  }

  std::string read_request() override {
    std::string raw;
    char buffer[1024];
    ssize_t n = 0;
    while ((n = read(client_fd_, buffer, sizeof(buffer))) > 0) {
      raw.append(buffer, buffer + n);
    }
    return raw;
  }

  bool send_response(const std::string &response) override {
    std::string payload = response;
    payload.push_back('\n');
    ssize_t written = write(client_fd_, payload.data(), payload.size());
    ::close(client_fd_);
    client_fd_ = -1;
    return written == static_cast<ssize_t>(payload.size());
  }

  void close() override {
    if (client_fd_ >= 0) {
      ::close(client_fd_);
      client_fd_ = -1;
    }
    if (server_fd_ >= 0) {
      ::close(server_fd_);
      server_fd_ = -1;
    }
  }

  unsigned int peer_uid() const override { return peer_uid_; }
  unsigned int peer_gid() const override { return peer_gid_; }
};

std::unique_ptr<IpcServer> create_ipc_server() {
  return std::make_unique<MacIpcServer>();
}

} // namespace helper
} // namespace ecnuvpn
```

- [ ] **Step 2: Verify macOS build with new file**

Add to CMakeLists.txt in the APPLE block:
```cmake
if(APPLE)
    # CommonCrypto links implicitly on macOS
    target_sources(exv PRIVATE helper_daemon_mac.cpp)
```
Run: `cmake -B build && cmake --build build -j$(sysctl -n hw.ncpu) 2>&1 | tail -5`
Expected: Build succeeds.

- [ ] **Step 3: Commit**

```bash
git add src/helper_daemon_mac.cpp CMakeLists.txt
git commit -m "feat: add macOS IpcServer implementation (AF_UNIX + getpeereid)"
```

---

### Task 2.3: Create Linux IpcServer implementation

**Files:**
- Create: `src/helper_daemon_linux.cpp`

- [ ] **Step 1: Write helper_daemon_linux.cpp**

```cpp
#include "helper_ipc.hpp"
#include "logger.hpp"

#include <cerrno>
#include <cstring>
#include <cstdio>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace ecnuvpn {
namespace helper {

class LinuxIpcServer : public IpcServer {
  int server_fd_ = -1;
  int client_fd_ = -1;
  unsigned int peer_uid_ = 0;
  unsigned int peer_gid_ = 0;

public:
  ~LinuxIpcServer() override { close(); }

  bool start(const std::string &path) override {
    server_fd_ = socket(AF_UNIX, SOCK_STREAM, 0);
    if (server_fd_ < 0)
      return false;

    sockaddr_un addr {};
    addr.sun_family = AF_UNIX;
    std::snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", path.c_str());

    if (bind(server_fd_, reinterpret_cast<sockaddr *>(&addr), sizeof(addr)) != 0) {
      ::close(server_fd_);
      server_fd_ = -1;
      return false;
    }

    // Linux: world-readable socket (any local user can manage VPN)
    chmod(path.c_str(), 0666);

    if (listen(server_fd_, 8) != 0) {
      ::close(server_fd_);
      server_fd_ = -1;
      std::remove(path.c_str());
      return false;
    }
    return true;
  }

  bool accept_client() override {
    client_fd_ = ::accept(server_fd_, nullptr, nullptr);
    if (client_fd_ < 0)
      return false;

    int flags = fcntl(client_fd_, F_GETFD);
    if (flags >= 0)
      fcntl(client_fd_, F_SETFD, flags | FD_CLOEXEC);

    return true;
  }

  bool verify_client() override {
    struct ucred cred;
    socklen_t cred_len = sizeof(cred);
    if (getsockopt(client_fd_, SOL_SOCKET, SO_PEERCRED, &cred, &cred_len) != 0) {
      ::close(client_fd_);
      client_fd_ = -1;
      return false;
    }
    peer_uid_ = cred.uid;
    peer_gid_ = cred.gid;
    return true;
  }

  std::string read_request() override {
    std::string raw;
    char buffer[1024];
    ssize_t n = 0;
    while ((n = read(client_fd_, buffer, sizeof(buffer))) > 0) {
      raw.append(buffer, buffer + n);
    }
    return raw;
  }

  bool send_response(const std::string &response) override {
    std::string payload = response;
    payload.push_back('\n');
    ssize_t written = write(client_fd_, payload.data(), payload.size());
    ::close(client_fd_);
    client_fd_ = -1;
    return written == static_cast<ssize_t>(payload.size());
  }

  void close() override {
    if (client_fd_ >= 0) {
      ::close(client_fd_);
      client_fd_ = -1;
    }
    if (server_fd_ >= 0) {
      ::close(server_fd_);
      server_fd_ = -1;
    }
  }

  unsigned int peer_uid() const override { return peer_uid_; }
  unsigned int peer_gid() const override { return peer_gid_; }
};

std::unique_ptr<IpcServer> create_ipc_server() {
  return std::make_unique<LinuxIpcServer>();
}

} // namespace helper
} // namespace ecnuvpn
```

- [ ] **Step 2: Update CMakeLists.txt for Linux**

```cmake
elseif(UNIX AND NOT APPLE)
    find_package(OpenSSL 1.1.1 REQUIRED)
    target_link_libraries(exv PRIVATE OpenSSL::SSL OpenSSL::Crypto)
    target_sources(exv PRIVATE helper_daemon_linux.cpp)
```

- [ ] **Step 3: Verify Linux build in Docker**

Run: `docker build -t exv-linux-test . 2>&1 | tail -10`
Expected: Build succeeds.

- [ ] **Step 4: Commit**

```bash
git add src/helper_daemon_linux.cpp CMakeLists.txt
git commit -m "feat: add Linux IpcServer implementation (AF_UNIX + SO_PEERCRED)"
```

---

### Task 2.4: Create Windows IpcServer implementation

**Files:**
- Create: `src/helper_daemon_win.cpp`

- [ ] **Step 1: Write helper_daemon_win.cpp**

```cpp
#include "helper_ipc.hpp"
#include "logger.hpp"

#ifdef _WIN32
#include <windows.h>
#endif

namespace ecnuvpn {
namespace helper {

class WinIpcServer : public IpcServer {
#ifdef _WIN32
  HANDLE hPipe_ = INVALID_HANDLE_VALUE;
  HANDLE hEvent_ = INVALID_HANDLE_VALUE;
  unsigned int peer_uid_ = 0;
  unsigned int peer_gid_ = 0;
#endif

public:
  ~WinIpcServer() override { close(); }

  bool start(const std::string &path) override {
#ifdef _WIN32
    hPipe_ = CreateNamedPipeA(
        path.c_str(),
        PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
        PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
        1, 65536, 65536, 0, NULL);

    if (hPipe_ == INVALID_HANDLE_VALUE) {
      logger::error("Helper: CreateNamedPipe failed");
      return false;
    }

    hEvent_ = CreateEvent(NULL, TRUE, FALSE, NULL);
    if (hEvent_ == INVALID_HANDLE_VALUE) {
      CloseHandle(hPipe_);
      hPipe_ = INVALID_HANDLE_VALUE;
      return false;
    }
    return true;
#else
    (void)path;
    return false;
#endif
  }

  bool accept_client() override {
#ifdef _WIN32
    OVERLAPPED ol = {};
    ol.hEvent = hEvent_;
    ResetEvent(hEvent_);

    ConnectNamedPipe(hPipe_, &ol);

    DWORD waitResult = WaitForSingleObject(hEvent_, INFINITE);
    if (waitResult != WAIT_OBJECT_0) {
      return false;
    }
    return true;
#else
    return false;
#endif
  }

  bool verify_client() override {
#ifdef _WIN32
    // Impersonate to get client identity
    if (!ImpersonateNamedPipeClient(hPipe_)) {
      logger::error("Helper: ImpersonateNamedPipeClient failed");
      return false;
    }

    // Get the impersonated token and extract SID
    HANDLE hToken = NULL;
    if (!OpenThreadToken(GetCurrentThread(), TOKEN_QUERY, FALSE, &hToken)) {
      RevertToSelf();
      return false;
    }

    // For now, allow all local connections
    // TODO: implement proper SID-based access check
    peer_uid_ = 0;
    peer_gid_ = 0;

    CloseHandle(hToken);
    RevertToSelf();
    return true;
#else
    return false;
#endif
  }

  std::string read_request() override {
#ifdef _WIN32
    std::string raw;
    char buffer[4096];
    DWORD bytesRead = 0;

    while (true) {
      BOOL success = ReadFile(hPipe_, buffer, sizeof(buffer) - 1, &bytesRead, NULL);
      if (!success || bytesRead == 0)
        break;
      raw.append(buffer, bytesRead);
    }
    return raw;
#else
    return "";
#endif
  }

  bool send_response(const std::string &response) override {
#ifdef _WIN32
    std::string payload = response;
    payload.push_back('\n');
    DWORD bytesWritten = 0;
    BOOL success = WriteFile(hPipe_, payload.c_str(),
                             static_cast<DWORD>(payload.size()), &bytesWritten, NULL);
    FlushFileBuffers(hPipe_);
    DisconnectNamedPipe(hPipe_);
    return success && bytesWritten == payload.size();
#else
    (void)response;
    return false;
#endif
  }

  void close() override {
#ifdef _WIN32
    if (hEvent_ != INVALID_HANDLE_VALUE) {
      CloseHandle(hEvent_);
      hEvent_ = INVALID_HANDLE_VALUE;
    }
    if (hPipe_ != INVALID_HANDLE_VALUE) {
      CloseHandle(hPipe_);
      hPipe_ = INVALID_HANDLE_VALUE;
    }
#endif
  }

  unsigned int peer_uid() const override { return peer_uid_; }
  unsigned int peer_gid() const override { return peer_gid_; }
};

std::unique_ptr<IpcServer> create_ipc_server() {
  return std::make_unique<WinIpcServer>();
}

} // namespace helper
} // namespace ecnuvpn
```

- [ ] **Step 2: Update CMakeLists.txt for Windows**

```cmake
elseif(WIN32)
    find_package(OpenSSL 1.1.1 REQUIRED)
    target_link_libraries(exv PRIVATE OpenSSL::SSL OpenSSL::Crypto)
    target_link_libraries(exv PRIVATE advapi32)
    target_sources(exv PRIVATE helper_daemon_win.cpp)
```

- [ ] **Step 3: Commit**

```bash
git add src/helper_daemon_win.cpp CMakeLists.txt
git commit -m "feat: add Windows IpcServer implementation (Named Pipe + SCM)"
```

---

### Task 2.5: Refactor daemon_main to use IpcServer

**Files:**
- Modify: `src/helper.cpp:1455-1577`

Replace the platform-specific daemon loop with a generic one using IpcServer.

- [ ] **Step 1: Replace daemon_main with IpcServer-based implementation**

Replace the body of `daemon_main()` (lines 1455-1577) with:

```cpp
int daemon_main() {
  signal(SIGTERM, daemon_signal_handler);
  signal(SIGINT, daemon_signal_handler);
  signal(SIGPIPE, SIG_IGN);

  auto ipc = create_ipc_server();

#ifdef _WIN32
  constexpr const char *ipc_path = "\\\\.\\pipe\\exv-helper";
#else
  constexpr const char *ipc_path = "/var/run/exv-helper.sock";
#endif

  remove_file_if_exists(ipc_path);

  if (!ipc->start(ipc_path)) {
    return 1;
  }

  while (!daemon_stop_requested) {
    reap_finished_request_handlers();

    if (!ipc->accept_client()) {
      if (daemon_stop_requested)
        break;
      continue;
    }

    if (!ipc->verify_client()) {
      continue;
    }

    std::string raw = ipc->read_request();
    unsigned int peer_uid = ipc->peer_uid();
    unsigned int peer_gid = ipc->peer_gid();

#ifndef _WIN32
    pid_t handler_pid = fork();
    if (handler_pid < 0) {
      nlohmann::json response =
          make_error("Failed to launch EXV helper request handler.");
      ipc->send_response(response.dump());
      continue;
    }

    if (handler_pid == 0) {
      signal(SIGTERM, SIG_DFL);
      signal(SIGINT, SIG_DFL);
      signal(SIGCHLD, SIG_DFL);
      signal(SIGPIPE, SIG_IGN);
      process_client_request(-1, peer_uid, peer_gid, raw);
      _exit(0);
    }
#else
    // Windows: process request inline (no fork)
    process_client_request(-1, peer_uid, peer_gid, raw);
    nlohmann::json response;
    try {
      nlohmann::json request = nlohmann::json::parse(raw);
      response = handle_request(peer_uid, peer_gid, request);
    } catch (...) {
      response = make_error("Failed to parse helper request.");
    }
    ipc->send_response(response.dump());
#endif
  }

  reap_finished_request_handlers();

  SessionState state;
  if (load_session_state(&state)) {
    std::string message;
    stop_managed_session(state, &message);
    clear_session_state();
  }

  ipc->close();
  remove_file_if_exists(ipc_path);
  return 0;
}
```

- [ ] **Step 2: Remove POSIX includes from helper.cpp that are now in daemon files**

Remove from the top of `helper.cpp` (if no longer needed by shared code):
```cpp
#include <sys/socket.h>
#include <sys/un.h>
```
Keep `<sys/wait.h>` and `<unistd.h>` (still used by shared code).

- [ ] **Step 3: Verify macOS build**

Run: `cmake -B build && cmake --build build -j$(sysctl -n hw.ncpu) 2>&1 | tail -5`
Expected: Build succeeds.

- [ ] **Step 4: Verify Linux build in Docker**

Run: `docker build -t exv-linux-test . 2>&1 | tail -10`
Expected: Build succeeds.

- [ ] **Step 5: Commit**

```bash
git add src/helper.cpp
git commit -m "refactor: daemon_main uses IpcServer abstraction"
```

---

### Task 2.6: Windows tunnel script generation

**Files:**
- Modify: `src/tunnel.cpp`

- [ ] **Step 1: Add Windows tunnel script header**

In the tunnel script generation function, add a Windows PowerShell shebang:

```cpp
#elif defined(_WIN32)
    ss << "# PowerShell tunnel script for Windows\n";
    ss << "$ErrorActionPreference = 'Stop'\n\n";
```

- [ ] **Step 2: Add Windows interface activation**

```cpp
#elif defined(_WIN32)
    ss << "# Interface activation handled by openconnect (WinTUN)\n";
    ss << "Write-Host \"[VPN] Interface $env:TUNDEV activated by openconnect\"\n";
```

- [ ] **Step 3: Add Windows route commands**

For each route block, replace the Windows `#elif defined(_WIN32)` sections with PowerShell `route.exe` commands:

```cpp
#elif defined(_WIN32)
    ss << "route delete " << server_ip << " 2>$null\n";
    ss << "$gw = (Get-NetRoute -DestinationPrefix '0.0.0.0/0' | Select-Object -First 1).NextHop\n";
    ss << "route add " << server_ip << " $gw\n";
```

For split tunnel routes:
```cpp
#elif defined(_WIN32)
    ss << "route delete " << route << " 2>$null\n";
    ss << "route add " << route << " $env:TUNDEV\n";
```

- [ ] **Step 4: Change tunnel script extension on Windows**

In `utils.cpp` `get_tunnel_path()`, add:
```cpp
#elif defined(_WIN32)
    return config_dir + "\\tunnel.ps1";
```

- [ ] **Step 5: Verify macOS build**

Run: `cmake --build build -j$(sysctl -n hw.ncpu) 2>&1 | tail -5`
Expected: Build succeeds.

- [ ] **Step 6: Commit**

```bash
git add src/tunnel.cpp src/utils.cpp
git commit -m "feat: add Windows PowerShell tunnel script generation"
```

---

### Task 2.7: Windows traffic statistics (GetIfEntry2)

**Files:**
- Modify: `src/utils.cpp`

- [ ] **Step 1: Replace Windows traffic stats stub with GetIfEntry2**

Find the Windows `get_interface_traffic()` stub and replace with:

```cpp
#elif defined(_WIN32)
    // Windows: use GetIfEntry2 from iphlpapi
    // Convert interface name to LUID
    NET_LUID luid;
    if (ConvertInterfaceNameToLuidA(iface.c_str(), &luid) != NO_ERROR)
      return false;

    MIB_IF_ROW2 row;
    ZeroMemory(&row, sizeof(row));
    row.InterfaceLuid = luid;
    if (GetIfEntry2(&row) != NO_ERROR)
      return false;

    *rx = row.InOctets;
    *tx = row.OutOctets;
    return true;
```

- [ ] **Step 2: Add iphlpapi to CMake Windows link libraries**

In CMakeLists.txt Windows block, add:
```cmake
    target_link_libraries(exv PRIVATE iphlpapi)
```

- [ ] **Step 3: Commit**

```bash
git add src/utils.cpp CMakeLists.txt
git commit -m "feat: implement Windows traffic stats via GetIfEntry2"
```

---

### Task 2.8: Adapt webui.cpp IPC for Windows Named Pipe

**Files:**
- Modify: `src/webui.cpp`

- [ ] **Step 1: Add platform-aware helper connection in status_broadcaster**

Find the `socket(AF_UNIX, ...)` call in webui.cpp (around line 534). Wrap it:

```cpp
#ifdef _WIN32
              // Windows: connect to Named Pipe
              // For now, return empty status (Named Pipe client not yet implemented)
              return "";
#else
              int fd = socket(AF_UNIX, SOCK_STREAM, 0);
              if (fd < 0) return "";

              struct sockaddr_un addr {};
              addr.sun_family = AF_UNIX;
              snprintf(addr.sun_path, sizeof(addr.sun_path), "/var/run/exv-helper.sock");
              // ... rest of existing POSIX code ...
#endif
```

- [ ] **Step 2: Commit**

```bash
git add src/webui.cpp
git commit -m "feat: add Windows Named Pipe stub in webui status broadcaster"
```

---

## Phase 3: Windows Functional

### Task 3.1: Windows install script

**Files:**
- Create: `scripts/install-win.ps1`

- [ ] **Step 1: Write PowerShell install script**

```powershell
#Requires -RunAsAdministrator
param()

$ErrorActionPreference = "Stop"

$installDir = "$env:ProgramFiles\ECNU-VPN"
$binaryDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$exvExe = Join-Path $binaryDir "exv.exe"

if (-not (Test-Path $exvExe)) {
    Write-Error "exv.exe not found in $binaryDir"
    exit 1
}

# Create install directory
New-Item -ItemType Directory -Force -Path $installDir | Out-Null

# Copy binary
Copy-Item -Force $exvExe (Join-Path $installDir "exv.exe")

# Add to PATH if not already present
$pathDirs = $env:PATH -split ";"
if ($installDir -notin $pathDirs) {
    $currentPath = [Environment]::GetEnvironmentVariable("PATH", "Machine")
    [Environment]::SetEnvironmentVariable("PATH", "$currentPath;$installDir", "Machine")
    Write-Host "Added $installDir to system PATH"
}

# Install helper service
& (Join-Path $installDir "exv.exe") service install

Write-Host "ECNU-VPN installed successfully." -ForegroundColor Green
Write-Host "Run 'exv' to start VPN connection."
```

- [ ] **Step 2: Commit**

```bash
git add scripts/install-win.ps1
git commit -m "feat: add Windows PowerShell install script"
```

---

## Phase 4: CI and Quality

### Task 4.1: GitHub Actions multi-platform CI

**Files:**
- Create: `.github/workflows/build.yml`

- [ ] **Step 1: Write CI workflow**

```yaml
name: Build

on: [push, pull_request]

jobs:
  build-macos:
    runs-on: macos-latest
    steps:
      - uses: actions/checkout@v4
      - name: Build
        run: |
          cmake -B build -DCMAKE_BUILD_TYPE=Release
          cmake --build build -j$(sysctl -n hw.ncpu)
      - name: Smoke test
        run: |
          ./build/exv version
          ./build/exv help

  build-linux:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install dependencies
        run: sudo apt-get install -y libssl-dev
      - name: Build
        run: |
          cmake -B build -DCMAKE_BUILD_TYPE=Release
          cmake --build build -j$(nproc)
      - name: Smoke test
        run: |
          ./build/exv version
          ./build/exv help

  build-windows:
    runs-on: windows-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install OpenSSL
        run: choco install openssl -y
      - name: Build
        run: |
          cmake -B build -DCMAKE_BUILD_TYPE=Release
          cmake --build build --config Release
      - name: Smoke test
        run: |
          .\build\Release\exv.exe version
          .\build\Release\exv.exe help
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/build.yml
git commit -m "ci: add macOS + Linux + Windows build workflow"
```

---

### Task 4.2: Docker multi-arch CI

**Files:**
- Modify: `.github/workflows/build.yml`

- [ ] **Step 1: Add Docker build job**

Append to `build.yml`:

```yaml
  build-docker:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Set up Docker Buildx
        uses: docker/setup-buildx-action@v3
      - name: Build Linux amd64
        run: docker build --platform linux/amd64 -t exv-linux-amd64 .
      - name: Build Linux arm64
        run: docker build --platform linux/arm64 -t exv-linux-arm64 .
```

- [ ] **Step 2: Commit**

```bash
git add .github/workflows/build.yml
git commit -m "ci: add Docker multi-arch build job"
```

---

## Self-Review Checklist

- [ ] Spec coverage: Every gap in the design doc has a corresponding task
- [ ] Placeholder scan: No TBD/TODO/implement-later in task steps
- [ ] Type consistency: IpcServer interface matches all three implementations
- [ ] Build verification: Each task includes a build verification step
