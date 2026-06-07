# De-File-ification: DAG-Organized Implementation Spec

**Date:** 2026-06-05
**Status:** Design Approved — Ready for parallel subagent execution

## DAG Overview

```
Phase 1 (4 parallel, no deps)
├── P1A: LogEventBus + Logger refactoring
├── P1B: Core Process named-pipe IPC (Channel B)
├── P1C: CoreCrashed UI (Electron)
└── P1D: Single-instance enforcement

Phase 2 (2 parallel, depends on P1A)
├── P2A: Remove log-tail consumers
└── P2B: Remove supervisor from vpn.cpp + vpn_runtime.cpp

Phase 3 (3 parallel, depends on P2A+P2B)
├── P3A: Remove native_session_store module
├── P3B: Remove PID/route-ready path utilities
└── P3C: Clean up helper daemon V1 file-based state

Phase 4 (depends on P1B+P3A+P3B+P3C)
└── P4: CLI thin-layer refactoring

Phase 5 (depends on P1B+P1C+P2B+P3A+P3C)
└── P5: Integration — wire everything, remove dead includes, verify build
```

```
P1A ──┬──→ P2A ──→ P3A ──────────────┐
      │         ↘                      │
      │           P2B ──→ P3B ──┐      │
      │                          │      │
P1B ──┼──────────────────────────┼──→ P4 ──→ P5
      │                          │      │
P1C ──┼──────────────────────────┤      │
      │                          │      │
P1D ──┘                          │      │
                                 │      │
                          P3C ───┘      │
                                 ↑      │
                    (P2A+P2B) ───┘      │
                                        │
          (P1B+P1C+P2B+P3A+P3C) ────────┘
```

---

## Phase 1: Independent Foundation (4 parallel tracks)

### P1A: LogEventBus + Logger Refactoring

**Goal:** Typed in-process event bus. Logger becomes write-only. LogRenderer bridges typed events → text → disk.

**New files:**
- `src/log_event_bus.hpp` — `TypedLogEvent` struct (level, component, code, message, fields map), `LogEventBus` singleton (publish/subscribe, thread-safe)
- `src/log_event_bus.cpp` — implementation
- `src/log_renderer.hpp` / `src/log_renderer.cpp` — subscribes to LogEventBus, renders `TypedLogEvent` → text line, calls `logger::write()`

**Modified files:**
- `src/logger.hpp` — add `logger::write(level, text)` (internal); keep `logger::tail(n)`; redirect `logger::info/error/warn/event()` through LogEventBus
- `src/logger.cpp` — `info/error/warn/event()` now publish to LogEventBus instead of writing directly; `write()` is the sole disk writer; `tail()` unchanged

**Dependencies:** None

**Verification:** `logger::info("test")` still produces a line in `ecnuvpn.log`; LogEventBus subscribers receive typed events; existing callers compile unchanged.

**Files:** `src/logger.hpp`, `src/logger.cpp`, `src/log_event_bus.hpp`, `src/log_event_bus.cpp`, `src/log_renderer.hpp`, `src/log_renderer.cpp`

---

### P1B: Core Process Named-Pipe IPC (Channel B)

**Goal:** Core Process accepts CLI connections via named pipe / Unix socket, using the same JSON-RPC protocol as Channel A (stdin/stdout).

**New files:**
- `src/core/pipe_ipc.hpp` / `src/core/pipe_ipc.cpp` — cross-platform pipe listener (Windows `CreateNamedPipe` + `ConnectNamedPipe`; Unix `socket`/`bind`/`listen`/`accept`), accept loop, read-one-line dispatch, write response, close

**Modified files:**
- `src/core/core_process.cpp` — integrate pipe listener into main event loop alongside stdin; pipe path: `\\.\pipe\exv-core` (Win) / `{state_dir}/exv-core.sock` (Unix)

**Dependencies:** None

**Verification:** Start `exv --mode=core`, connect via pipe client, send `{"action":"status.get"}`, receive valid JSON response. Multiple sequential connections work.

**Files:** `src/core/pipe_ipc.hpp`, `src/core/pipe_ipc.cpp`, `src/core/core_process.cpp`

---

### P1C: CoreCrashed UI (Electron)

**Goal:** When Core Process exits unexpectedly, Electron shows a full-screen "Core Crashed" overlay with restart/quit options. No silent auto-restart.

**Modified files:**
- `webui/desktop/main/index.ts` — `CoreProcessManager`: remove `scheduleRestart()` / exponential backoff; on `exit` event, push `{"event":"core-crashed","data":{exitCode, signal}}` to renderer; add `restart()` method for manual restart
- `webui/src/App.vue` — route to CoreCrashed overlay on `core-crashed` event

**New files:**
- `webui/src/components/CoreCrashed.vue` — overlay: exit code display, "重启内核" and "退出程序" buttons

**Dependencies:** None

**Verification:** Kill core → CoreCrashed overlay shown. "重启内核" restarts core. "退出程序" quits app. Normal disconnect does NOT trigger.

**Files:** `webui/desktop/main/index.ts`, `webui/src/components/CoreCrashed.vue`, `webui/src/App.vue`

---

### P1D: Single-Instance Enforcement

**Goal:** Only one `exv --mode=core` process at a time. Enforced via pipe exclusivity.

**Modified files:**
- `src/core/core_process.cpp` — at startup, attempt to create/bind the named pipe; if it fails, write error to stderr and `exit(1)`. This doubles as the Channel B listener from P1B.

**Dependencies:** P1B (uses same pipe for both guard AND CLI IPC)

**Verification:** Second `exv --mode=core` exits immediately with error. Kill first → new one succeeds.

**Files:** `src/core/core_process.cpp`

---

## Phase 2: Consumer Removal (2 parallel, depend on P1A)

### P2A: Remove Log-Tail Consumers + SSE Broadcaster

**Goal:** Delete all modules that read `ecnuvpn.log` to determine system state. Delete SSE broadcaster entirely (WebUI is retired; Electron is the only GUI).

**Deleted files:**
- `src/sse_broadcaster.cpp` — entire file (log watcher, status poller, heartbeat — all WebUI-only)
- `src/sse_broadcaster.hpp` — entire file

**Modified files:**
- `src/vpn.cpp` — delete `AuthFailureWatch` class, `RuntimeLogTail` class, all usages in `run_supervisor()`
- `src/tunnel.cpp` — delete `runtime_log_has_auth_failure()` (lines ~206-212)
- `src/tunnel.hpp` — delete declaration

**Dependencies:** P1A (LogEventBus must exist before removing consumers)

**Verification:** `AuthFailureWatch`, `RuntimeLogTail`, `runtime_log_has_auth_failure()`, `SseBroadcaster` no longer exist. Build succeeds.

**Files:** Delete `src/sse_broadcaster.cpp`, `src/sse_broadcaster.hpp`; modify `src/vpn.cpp`, `src/tunnel.cpp`, `src/tunnel.hpp`

---

### P2B: Remove Supervisor from vpn.cpp + vpn_runtime.cpp

**Goal:** Delete the legacy supervisor path entirely.

**Modified files — `src/vpn.cpp`:** Delete: `ConnectionDiagnostics`, `LoggingEventSink`, `write_pid_file`, `read_pid_file`, `remove_pid_file`, `write_pid`, `read_pid`, `remove_pid`, `write_supervisor_pid`, `read_supervisor_pid`, `remove_supervisor_pid`, `remove_route_ready`, `read_route_ready`, `clear_runtime_state`, `is_process_alive`, `find_openconnect_pid`, `terminate_process`, `sleep_ms`, `append_openconnect_attempt_marker`, `handle_supervisor_signal`, `describe_retry_policy`, `run_native_supervisor()`, `run_supervisor()`, `supervisor_main()`. In `start()`: remove old supervisor branches, keep only TunnelController path.

**Modified files — `src/vpn.hpp`:** Remove deleted function declarations.

**Modified files — `src/vpn_runtime.cpp`:** Delete local `read_pid_file()`, `read_route_ready()`, `is_process_alive()`, `find_openconnect_pid()`. Rewrite `read_runtime_status_snapshot()` — remove file-based paths; query Core Process via pipe or return "unknown" if core not running.

**Dependencies:** P1A (LogEventBus replaces LoggingEventSink)

**Verification:** No supervisor functions exist. No references to PID files or route-ready in vpn.cpp. Build succeeds.

**Files:** `src/vpn.cpp`, `src/vpn.hpp`, `src/vpn_runtime.cpp`

---

## Phase 3: Deep Cleanup (3 parallel, depend on P2A+P2B)

### P3A: Remove native_session_store Module

**Goal:** Delete the entire module after all consumers are removed.

**Deleted files:**
- `src/vpn_engine/native_session_store.cpp`
- `src/vpn_engine/native_session_store.hpp`

**Modified files (remove #include and usages):**
- `src/vpn.cpp` — remove `#include "vpn_engine/native_session_store.hpp"`
- `src/helper.cpp` — remove `#include` and all usages (see P3C)
- `src/app_api.cpp` — remove `#include`; delete `cleanup_legacy_supervisor_state_files()` function
- `src/vpn_runtime.cpp` — remove `#include`

**Preserved data structures (move to `src/vpn_engine/session_state.hpp`):**
- `SessionPhase` enum, `SessionState` struct, `TunnelMetadata` struct

**Dependencies:** P2A + P2B

**Verification:** Files deleted. No `#include "vpn_engine/native_session_store.hpp"` anywhere. Data structures preserved. Build succeeds.

**Files:** Delete 2; modify `src/vpn.cpp`, `src/helper.cpp`, `src/app_api.cpp`, `src/vpn_runtime.cpp`

---

### P3B: Remove PID/Route-Ready Path Utilities

**Goal:** Delete path getters for files that no longer exist.

**Modified files:**
- `src/utils.cpp` — delete `get_pid_path()`, `get_supervisor_pid_path()`, `get_route_ready_path()`
- `src/utils.hpp` — delete declarations
- `src/platform/common/path_utils.hpp` — delete `pid_path()`, `supervisor_pid_path()`, `route_ready_path()`
- `src/platform/{darwin,linux,win32}/path_utils.cpp` — delete implementations

**Note on `get_tunnel_path()`:** Used by `tunnel.cpp` for tunnel script generation. Keep if legacy_openconnect engine still needs tunnel scripts; otherwise delete.

**Dependencies:** P2A + P2B

**Verification:** No references to deleted path getters. Build succeeds.

**Files:** `src/utils.cpp`, `src/utils.hpp`, `src/platform/common/path_utils.hpp`, 3× platform `path_utils.cpp`

---

### P3C: Clean Up Helper Daemon V1 File-Based State

**Goal:** Helper daemon becomes purely a V2 session-based privileged operation executor. Remove all V1 actions that depend on PID files, route-ready, and native-session-state.json.

**Modified files — `src/helper.cpp`:**
Delete: `pid_path_for()`, `supervisor_pid_path_for()`, `route_ready_path_for()`, `clear_runtime_state()`, `read_pid_file()`, `read_route_ready()`, `is_process_alive()`, `find_openconnect_pid()`, `inspect_runtime()`, `save_session_state()`, `load_session_state()`, `clear_session_state()`, `clear_native_session_state_for_known_config_dirs()`, `stop_managed_session()`, `handle_start()`, `handle_stop()`, `handle_status()`, `make_status_response()`, `print_running_status()`, `create_request_file()`. In `handle_request()`: remove V1 action dispatch (start/stop/status). In `daemon_main()` accept loop: remove V1 request handling.

Keep: V2 handler dispatch (`helper_v2_handler.hpp`), `make_hello_response()`, `make_helper_descriptor()`, `make_helper_capabilities()`, `make_error()`, `daemon_main()` IPC setup.

**Modified files — `src/helper.hpp`:** Remove `start_via_helper()`, `stop_via_helper()`, `status_via_helper()` if present. Keep `is_available()`, `daemon_main()`, `worker_main()`, `install_service()`, `uninstall_service()`.

**Dependencies:** P2A + P2B

**Verification:** Helper daemon starts and responds to V2 Hello/StartSession. V1 actions return error or are removed. No references to PID files, route-ready, native-session-state.json in helper.cpp. Build succeeds.

**Files:** `src/helper.cpp`, `src/helper.hpp`

---

## Phase 4: CLI Thin-Layer Refactoring + WebUI Removal

**Goal:** CLI becomes a thin frontend that connects to Core Process via named pipe, sends RPC, displays result, exits. No embedded VPN logic. WebUI is permanently removed — Electron is the sole GUI entry point.

**Deleted files:**
- `src/webui.cpp` / `src/webui.hpp` — entire WebUI HTTP server module

**Modified files:**
- `src/main.cpp` — rewrite CLI command handlers:
  - `start` → connect to core pipe → send `{"action":"vpn.connect",...}` → display result → exit
  - `stop` → connect to core pipe → send `{"action":"vpn.disconnect"}` → display result → exit
  - `status` → connect to core pipe → send `{"action":"status.get"}` → format and print → exit
  - `config` subcommands → connect to core pipe → send config.* RPC → display → exit
  - `logs` → connect to core pipe → send `{"action":"logs.list",...}` → print lines → exit
  - `service` subcommands → connect to core pipe → send service.* RPC → display → exit
- CLI core-lazying: if pipe connect fails → spawn `exv --mode=core` → poll pipe → retry connect → proceed
- Remove `desktop-rpc` / `desktop-rpc-file` / `desktop-rpc-file-output` entry points (Electron now uses stdin/stdout exclusively)
- Remove `__helper-daemon`, `__tunnel-script`, `__helper-exec`, `__vpn-supervisor` entry points
- Remove all WebUI foreground/background mode logic (`--foreground`, `-f` flag, fork/daemonize, SSE broadcaster instantiation)
- Remove `webui_enabled`, `webui_port`, `webui_bind` config fields (or deprecate with warning)

**New files:**
- `src/cli/pipe_client.hpp` / `src/cli/pipe_client.cpp` — thin wrapper: connect to core pipe, send one JSON-RPC request, receive response, disconnect

**Dependencies:** P1B (pipe IPC must exist), P3A+P3B+P3C (file-based paths must be gone)

**Verification:** `exv status` works without core running (auto-spawns core). `exv start` sends connect request and exits immediately (does not block). `exv logs` shows log lines. `exv config show` displays config.

**Files:** Delete `src/webui.cpp`, `src/webui.hpp`; modify `src/main.cpp`; new `src/cli/pipe_client.hpp`, `src/cli/pipe_client.cpp`

---

## Phase 5: Integration & Final Cleanup

**Goal:** Wire everything together, remove dead includes, verify full build across all platforms.

**Actions:**
1. Audit all `#include` directives — remove references to deleted headers
2. Update `CMakeLists.txt` — remove deleted source files (`native_session_store`, `sse_broadcaster`, `webui`), add new ones (`log_event_bus`, `log_renderer`, `pipe_ipc`, `cli/pipe_client`)
3. Verify `src/main.cpp` entry point dispatch is correct (only `--mode=core` and CLI commands remain)
4. Remove `webui_enabled`, `webui_port`, `webui_bind` from `Config` struct and config serialization (or emit deprecation warning if present)
5. Cross-platform build verification: `cmake --build build` on macOS, Windows, Linux
6. Run existing test suite, update tests that reference deleted functions
7. Update `docs/` architecture docs to reflect new topology

**Dependencies:** P1B, P1C, P2B, P3A, P3C, P4

**Verification:** Clean build on all platforms. All tests pass (or updated). No dead includes. No references to deleted symbols. `exv --help` shows only CLI commands and `--mode=core`.

**Files:** `CMakeLists.txt`, `src/main.cpp`, `src/config.hpp`, `src/config.cpp`

---

## Cross-Reference Map: Who Touches What

| File | Phases | Conflict Risk |
|------|--------|---------------|
| `src/vpn.cpp` | P2A, P2B, P3A | **HIGH** — P2A and P2B both delete from this file. P2B must run AFTER P2A, or coordinate on a single edit pass |
| `src/vpn_runtime.cpp` | P2B, P3A | MEDIUM — P2B rewrites, P3A removes include |
| `src/helper.cpp` | P3A, P3C | MEDIUM — P3A removes include, P3C deletes functions
| `src/main.cpp` | P4, P5 | MEDIUM — P4 rewrites entry points, P5 cleans up includes |
| `src/config.hpp` / `src/config.cpp` | P5 | LOW — remove deprecated WebUI config fields |

## Deleted Files Summary

| File | Phase | Reason |
|------|-------|--------|
| `src/vpn_engine/native_session_store.cpp` | P3A | File-based state persistence |
| `src/vpn_engine/native_session_store.hpp` | P3A | File-based state persistence |
| `src/sse_broadcaster.cpp` | P2A | WebUI-only; Electron uses JSON-RPC push |
| `src/sse_broadcaster.hpp` | P2A | WebUI-only |
| `src/webui.cpp` | P4 | WebUI retired; Electron is sole GUI |
| `src/webui.hpp` | P4 | WebUI retired |

## Runtime Files Eliminated

| File | Former Writer | Fate |
|------|--------------|------|
| `native-session-state.json` | `NativeSessionEventRecorder` | No longer written |
| `route-ready` | `write_route_ready_marker()` | No longer written |
| `ecnuvpn.pid` | Supervisor | No longer written |
| `ecnuvpn-supervisor.pid` | Supervisor | No longer written |
