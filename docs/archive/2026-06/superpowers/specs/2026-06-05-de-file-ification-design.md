# De-File-ification: Eliminating File-Based Runtime Communication

**Date:** 2026-06-05
**Status:** Design Approved

## 1. Problem Statement

The ECNU-VPN system currently relies heavily on file-based communication for runtime state. Four runtime files act as ad-hoc IPC channels between processes:

| File | Writer | Reader | Purpose |
|------|--------|--------|---------|
| `native-session-state.json` | `NativeSessionEventRecorder` (supervisor) | `read_native_session_snapshot()` (status CLI/Electron) | Session phase, PID, tunnel metadata, failure codes |
| `route-ready` | `write_route_ready_marker()` | Supervisor poll loop, status commands | Tunnel interface name + internal IP |
| `ecnuvpn.pid` / `ecnuvpn-supervisor.pid` | Supervisor | Status commands | Process liveness check |
| `ecnuvpn.log` | `logger::info/error/warn/event()` | SSE broadcaster (kqueue/inotify tail), `AuthFailureWatch` | Real-time log streaming + auth failure detection |

Additionally, the log file is used as a **side-channel for state detection** — `AuthFailureWatch` tails the log to detect authentication failures, and `SseBroadcaster::log_watcher()` parses log lines to reconstruct structured events for the UI. This is reverse-engineering: serialize to text, then parse text back.

## 2. Design Principles

1. **Internal communication uses typed events, not text.** Modules communicate with Core via strongly-typed C++ structs/enums. Core translates to human-readable text only for the log file.
2. **The log file is write-only for diagnostics.** Only `exv logs` (CLI) and the Electron log viewer may read it. No module reads the log to determine system state.
3. **Core Process is the single source of truth.** All state lives in memory in `TunnelController`. All consumers query Core via JSON-RPC.
4. **CLI and Electron are equal "frontends."** Both are thin UI layers that translate user actions to RPC calls and display results. CLI is short-lived and disposable.
5. **Core is single-instance.** Enforced via named pipe / Unix socket exclusivity.
6. **Core crashes are user-visible.** No silent auto-restart. The UI presents a clear "Core crashed" screen with options to restart or quit.

## 3. Target Architecture

```
┌──────────────────┐     ┌──────────────────────┐
│ Electron (UI)    │     │ CLI (exv start/stop/  │
│  long-lived      │     │  status/config/logs)  │
│                  │     │  short-lived,即用即销  │
│ CoreProcessMgr   │     │                      │
│  spawns core     │     │ 若 core 未运行→拉起    │
│  stdin/stdout    │     │ 若 core 已运行→连 pipe │
└────────┬─────────┘     └──────────┬───────────┘
         │                          │
         │ child_process            │ named pipe / Unix socket
         │ stdin/stdout JSON-RPC    │ connect → send → recv → exit
         │                          │
         ▼                          ▼
┌────────────────────────────────────────────────┐
│ exv --mode=core  (Core Process)               │
│  单实例 · 长命 · 唯一状态持有者                 │
│                                                │
│  ├─ TunnelController (in-memory state machine) │
│  ├─ JSON-RPC over stdin/stdout  ← Electron 通道│
│  ├─ JSON-RPC over named pipe    ← CLI 通道     │
│  ├─ LogEventBus → typed events → LogRenderer   │
│  └─ Logger → ecnuvpn.log (write-only, 纯诊断)  │
└────────────────────┬───────────────────────────┘
                     │ named pipe / Unix socket
                     ▼
┌────────────────────────────────────────────────┐
│ exv-helper (privileged daemon)                 │
│  单实例 · 仅做特权网络操作 · 不持有会话状态      │
│  ├─ Tunnel device create/destroy               │
│  ├─ Route / DNS apply/remove                   │
│  └─ Session lease heartbeat                    │
└────────────────────────────────────────────────┘
```

### Process Responsibilities

| Process | Lifecycle | Responsibility |
|---------|-----------|----------------|
| Electron / CLI | Long-lived / Short-lived | User interaction → translate to RPC → display results |
| Core Process | Long-lived, single-instance | State machine, business logic, coordinate helper |
| Helper Daemon | Long-lived, single-instance | Privileged network operations (TUN, routes, DNS) |

### CLI Workflow

```
User runs: exv status
  → CLI process starts
  → Attempt connect to core's named pipe
  → If connection fails → spawn exv --mode=core, wait for pipe ready
  → Send {"action":"status.get"} via pipe
  → Receive JSON response
  → Format and print to terminal
  → CLI process exits
```

## 4. IPC Design

### 4.1 Dual-Channel JSON-RPC

Core Process serves two client types using identical JSON-RPC protocol:

| Channel | Client | Lifecycle | Transport |
|---------|--------|-----------|-----------|
| Channel A | Electron | Long-lived, co-lifetime with core | child_process stdin/stdout |
| Channel B | CLI | Short-lived, connect-send-disconnect | Named pipe (Win) / Unix socket (Unix) |

Both channels use the same message format (`RpcRequest`/`RpcResponse`) and dispatch through the same `AppRpcDispatcher`.

### 4.2 Channel A (Electron)

- Electron's `CoreProcessManager` spawns `exv --mode=core`
- Requests: Electron → stdin, one JSON line
- Responses: stdout → Electron, one JSON line
- Event push: Core writes `{"event":"status","data":{...}}` to stdout; `CoreRpcClient.handleLine()` dispatches to `eventListeners`
- Existing `CoreRpcClient` implementation is reused

### 4.3 Channel B (CLI)

- Core creates a listening pipe/socket at a well-known path on startup
- Pipe path: `\\.\pipe\exv-core` (Windows) / `{state_dir}/exv-core.sock` (Unix)
- CLI connects → sends one request → receives response → disconnects → exits
- Core uses accept loop: accept, read one line, dispatch, write response, close

### 4.4 Single-Instance Enforcement

**Core Process:**
```
exv --mode=core starts
  → Attempt to create/bind named pipe (exv-core)
  → Success: sole instance, proceed
  → Failure (pipe already bound): another core is running
      → Write error to stderr
      → exit(1)
```

**CLI core-lazying logic:**
```
CLI starts
  → connect(pipe_path)
  → Success → send RPC → receive response → exit
  → Failure (ECONNREFUSED / ERROR_FILE_NOT_FOUND)
      → spawn("exv --mode=core", detached)
      → Poll connect(pipe_path) up to N seconds
      → Success → send RPC → receive response → exit
      → Timeout → report error, exit
```

**Helper Daemon:** Single-instance already guaranteed by system service manager. No change.

### 4.5 Core Main Loop Structure

```
core_process_main():
  bootstrap()
  logger::init()

  create TunnelController(helper_client, net_ops)
  create RpcDispatcher(controller)
  create LogEventBus → subscribe LogRenderer → Logger

  bind named pipe (Channel B)

  // Main event loop
  while !stop_requested:
    select/poll on:
      [1] stdin  (Channel A — Electron)
      [2] pipe listener (Channel B — new CLI connections)
      [3] pipe clients (Channel B — active CLI connections)
      [4] signal fd / stop flag

    if stdin ready:
      read line → dispatch → write response to stdout

    if pipe listener ready:
      accept new client → add to poll set

    if pipe client ready:
      read line → dispatch → write response → close client

    if TunnelController status changed:
      push event to stdout (Channel A)
```

## 5. Logger Refactoring

### 5.1 Current Problem

`SseBroadcaster::log_watcher()` uses OS file monitoring (kqueue/inotify/ReadDirectoryChangesW) to tail `ecnuvpn.log`, then `parse_and_broadcast_log_line()` reverse-parses text lines to reconstruct structured events. `AuthFailureWatch` tails the same log to detect authentication failure text patterns. This is serialization followed by deserialization — fragile and inefficient.

### 5.2 Target Design

```
Internal Modules
  │
  │  TypedEvent (C++ struct/enum)
  ▼
┌──────────────────┐
│   LogEventBus    │  ← In-process pub/sub
│  (typed events)  │
└────┬─────────────┘
     │
     │ subscribe
     ▼
┌──────────────────┐     text line     ┌─────────┐
│  LogRenderer     │ ───────────────→  │ Logger  │──→ ecnuvpn.log
│  (typed→text)    │                   │ (write  │
└──────────────────┘                   │  only)  │
                                       └─────────┘
     │
     │ subscribe (for JSON-RPC push)
     ▼
┌──────────────────┐
│  Core Process    │──→ Electron stdout: {"event":"log","data":{...}}
│  JSON-RPC layer  │──→ CLI pipe: logs.list response
└──────────────────┘
```

### 5.3 Logger Module (Reduced Scope)

- `logger::init()` — ensure log directory exists
- `logger::write(level, text)` — append one line to `ecnuvpn.log` (internal, not exposed)
- `logger::tail(n)` — read last N lines (only called by `logs.list` RPC handler)

### 5.4 New: LogEventBus

- In-process singleton, publishes `TypedLogEvent` (strongly-typed struct with level, component, code, message, fields map)
- Subscribers: `LogRenderer` (converts to text → `logger::write()`), Core Process JSON-RPC layer (converts to JSON → pushes to Electron)
- No file I/O involved

### 5.5 Deleted Modules

- `SseBroadcaster::log_watcher()` — entire file-tail thread (kqueue/inotify/ReadDirectoryChangesW)
- `SseBroadcaster::parse_and_broadcast_log_line()` — text line reverse-parsing
- `AuthFailureWatch` class — tail log for auth failure detection
- `RuntimeLogTail` class — generic log tail utility
- `tunnel::runtime_log_has_auth_failure()` — read log to determine state

## 6. Core Crash Handling

### 6.1 Electron Behavior

```
CoreProcessManager detects child_process 'exit' event
  → Do NOT auto-restart (remove scheduleRestart / exponential backoff)
  → Push event to renderer: {"event":"core-crashed","data":{"exitCode":N,"signal":S}}
  → Renderer switches to "CoreCrashed" full-screen overlay
```

### 6.2 UI: CoreCrashed View

Full-window overlay with only:

- ⚠ 内核意外退出
- Explanation: "VPN 核心进程已终止，VPN 连接可能已中断。"
- Exit code display
- Two buttons: **重启内核** | **退出程序**

- **重启内核**: `CoreProcessManager.start()` re-spawns core, on success restore normal UI
- **退出程序**: `app.quit()`
- All other UI interaction is blocked while this view is shown

### 6.3 Logging for Crash Diagnosis

Logger must record sufficient detail for post-crash analysis:

- Core startup: version, platform, config path, helper connection status
- Every RPC request/response: action + duration + result
- Every TunnelController state transition: idle → authenticating → ... → failed, with error code
- Engine event full fields
- Any exception/error path with full context

These records are written to disk. After a crash, users can retrieve them via `exv logs` or the Electron log viewer.

## 7. Complete Deletion Inventory

### 7.1 C++ Source Files (Delete Entirely)

| File | Reason |
|------|--------|
| `src/vpn_engine/native_session_store.cpp` | All file-based state read/write logic |
| `src/vpn_engine/native_session_store.hpp` | `NativeSessionEventRecorder`, save/load/clear/snapshot functions |

### 7.2 C++ Source Files (Heavily Trim)

| File | Removals |
|------|----------|
| `src/sse_broadcaster.cpp` | `log_watcher()`, `parse_and_broadcast_log_line()`; possibly entire file if WebUI is also removed |
| `src/sse_broadcaster.hpp` | Corresponding declarations |

### 7.3 Functions/Classes to Delete Within Files

**`src/vpn.cpp`:**
- `run_native_supervisor()`
- `run_supervisor()`
- `RuntimeLogTail` class
- `AuthFailureWatch` class
- `ConnectionDiagnostics` class
- `LoggingEventSink` class
- `write_pid_file()`, `read_pid_file()`, `remove_pid_file()`
- `write_pid()`, `read_pid()`, `remove_pid()`
- `write_supervisor_pid()`, `read_supervisor_pid()`, `remove_supervisor_pid()`
- `remove_route_ready()`, `read_route_ready()`
- `clear_runtime_state()`
- `is_process_alive()`, `find_openconnect_pid()`, `terminate_process()`, `sleep_ms()`
- `append_openconnect_attempt_marker()`
- `handle_supervisor_signal()`, `describe_retry_policy()`
- `supervisor_main()`
- Old supervisor branches in `start()`

**`src/vpn.hpp`:** Corresponding declarations.

**`src/utils.cpp` / `src/utils.hpp`:**
- `get_pid_path()`, `get_supervisor_pid_path()`, `get_route_ready_path()`, `get_tunnel_path()`

**`src/platform/common/path_utils.hpp` + platform impls:**
- `pid_path()`, `supervisor_pid_path()`, `route_ready_path()`, `tunnel_path()`

**`src/tunnel.cpp` / `src/tunnel.hpp`:**
- `runtime_log_has_auth_failure()`

**`src/main.cpp`:**
- WebUI foreground/background supervisor logic
- `__vpn-supervisor` entry point

### 7.4 Runtime Files (No Longer Produced)

| File | Fate |
|------|------|
| `native-session-state.json` | No longer written |
| `route-ready` | No longer written |
| `ecnuvpn.pid` | No longer written |
| `ecnuvpn-supervisor.pid` | No longer written |

### 7.5 Electron/TypeScript Changes

| File | Change |
|------|--------|
| `webui/desktop/main/index.ts` | Remove `CoreProcessManager` auto-restart; add `core-crashed` event handling; remove legacy `execFile` fallback |
| `webui/desktop/main/core-rpc-client.ts` | Reuse as-is |
| `webui/src/` | New `CoreCrashed` view component |

### 7.6 Preserved but Repurposed

| Module | New Role |
|--------|----------|
| `logger::tail(n)` | Only serves `logs.list` RPC |
| `SessionState` / `SessionPhase` / `TunnelMetadata` | Internal to TunnelController, never serialized to file |
| `TunnelStatusSnapshot` | Already in TunnelController, exposed via JSON-RPC |
| `ConfigManager` | Unchanged (config persistence to disk is legitimate) |
| `ecnuvpn.log` | Write-only diagnostic log; only read by `exv logs` and Electron log viewer |

## 8. Summary

| Dimension | Current | Target |
|-----------|---------|--------|
| State communication | 4 runtime files | 0, all via JSON-RPC |
| Log purpose | Write + tail for state detection | Write-only, pure diagnostics |
| Old supervisor path | Present, primary | Deleted entirely |
| CLI positioning | Embedded VPN logic | Thin UI, connects to Core pipe |
| Core instances | Unlimited | Single-instance enforced |
| Core crash handling | Silent auto-restart | Explicit UI, user decides |
| Internal events | Text log lines | Typed events + LogEventBus |
