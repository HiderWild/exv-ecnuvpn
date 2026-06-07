# De-File-ification Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Eliminate all file-based runtime communication. Replace with typed in-process events (LogEventBus), JSON-RPC over stdin/stdout (Electron) and named pipe/Unix socket (CLI). Remove legacy supervisor, WebUI, SSE broadcaster, and all PID/route-ready/session-state files.

**Architecture:** Three-process topology — Electron (long-lived GUI) and CLI (short-lived, disposable) are equal frontends connecting to a single-instance Core Process (`exv --mode=core`) via JSON-RPC. Core holds all state in-memory (TunnelController). Helper daemon is a pure V2 privileged operation executor. Logger is write-only; LogEventBus carries typed events internally.

**Tech Stack:** C++17 (CMake, nlohmann/json), TypeScript (Electron + Vue 3 + Vite), pnpm for Node workflows.

**Specs:** `docs/superpowers/specs/2026-06-05-de-file-ification-design.md`, `docs/superpowers/specs/2026-06-05-de-file-ification-dag.md`

---

## Phase 1: Independent Foundation (4 parallel tracks)

### Task 1A: LogEventBus + Logger Refactoring

**Files:** Create `src/log_event_bus.hpp`, `src/log_event_bus.cpp`, `src/log_renderer.hpp`, `src/log_renderer.cpp`. Modify `src/logger.hpp`, `src/logger.cpp`, `src/core/core_process.cpp`.

**Dependencies:** None

- [ ] **Step 1: Create `src/log_event_bus.hpp`** — `TypedLogEvent` struct and `LogEventBus` singleton with `subscribe`/`unsubscribe`/`publish`. Thread-safe via `std::mutex`.

- [ ] **Step 2: Create `src/log_event_bus.cpp`** — Implement the singleton pattern, subscription management, and publish loop.

- [ ] **Step 3: Create `src/log_renderer.hpp`** — RAII class that subscribes to LogEventBus on construction, unsubscribes on destruction.

- [ ] **Step 4: Create `src/log_renderer.cpp`** — `render()` function converts `TypedLogEvent` to `[timestamp] [LEVEL] [component] code=... message key=value` text line. Subscription callback calls `logger::write(level, rendered_text)`.

- [ ] **Step 5: Modify `src/logger.hpp`** — Add `void write(const std::string& level, const std::string& text)` declaration after `init()`. This is the sole disk writer, called only by LogRenderer.

- [ ] **Step 6: Modify `src/logger.cpp`** — Remove old `write_log()` static function. `info/error/warn/event()` now construct `TypedLogEvent` and call `LogEventBus::instance().publish()`. New `write()` function contains the disk I/O (open file, append, flush, sync_owner). `tail()` and `show_logs()` unchanged. Add `#include "log_event_bus.hpp"`.

- [ ] **Step 7: Instantiate LogRenderer in `core_process_main()`** — In `src/core/core_process.cpp`, after `logger::init()`, add `ecnuvpn::LogRenderer log_renderer;` and `#include "log_renderer.hpp"`.

- [ ] **Step 8: Build and verify** — `cmake --build build --target exv` succeeds. `logger::info("test")` still writes to `ecnuvpn.log`.

- [ ] **Step 9: Commit** — `git add` all new/modified files, commit with message `"feat: add LogEventBus and refactor Logger to write-only"`.

---

### Task 1B: Core Process Named-Pipe IPC (Channel B)

**Files:** Create `src/core/pipe_ipc.hpp`, `src/core/pipe_ipc.cpp`. Modify `src/core/core_process.cpp`.

**Dependencies:** None

- [ ] **Step 1: Create `src/core/pipe_ipc.hpp`** — `PipeIpcListener` class with `start()`/`stop()`/`accept_one(handler)`. `core_pipe_path()` returns `\\.\pipe\exv-core` (Win) or `{state_dir}/exv-core.sock` (Unix).

- [ ] **Step 2: Create `src/core/pipe_ipc.cpp`** — Cross-platform implementation:
  - Windows: `CreateNamedPipeA` with `FILE_FLAG_FIRST_PIPE_INSTANCE` + `PIPE_ACCESS_DUPLEX`, `ConnectNamedPipe`, `ReadFile`/`WriteFile`, `DisconnectNamedPipe`
  - Unix: `socket(AF_UNIX, SOCK_STREAM)`, `bind`, `listen`, `accept` (non-blocking via `fcntl O_NONBLOCK`), `read`/`write`, `close`
  - `accept_one()`: check for pending connection, read one line, call handler, write response + newline, close client

- [ ] **Step 3: Integrate into `core_process_main()`** — After LogRenderer instantiation, create and start `PipeIpcListener`. In main event loop, after stdin processing, call `pipe_listener->accept_one(dispatch_fn)` where `dispatch_fn` parses JSON-RPC request, dispatches to `AppRpcDispatcher`, returns JSON response line.

- [ ] **Step 4: Build and verify** — `cmake --build build --target exv`. Start `exv --mode=core`, test pipe connection: `echo '{"action":"status.get"}' | nc -U /path/to/exv-core.sock` (Unix) or pipe client (Win). Expected: valid JSON-RPC response.

- [ ] **Step 5: Commit** — `git add` new/modified files, commit `"feat: add named-pipe IPC for CLI connections to core process"`.

---

### Task 1C: CoreCrashed UI (Electron)

**Files:** Create `webui/src/components/CoreCrashed.vue`. Modify `webui/desktop/main/index.ts`, `webui/src/App.vue`.

**Dependencies:** None

- [ ] **Step 1: Create `webui/src/components/CoreCrashed.vue`** — Full-screen overlay with warning icon, "内核意外退出" heading, exit code display, "重启内核" (primary) and "退出程序" (secondary) buttons. Dark overlay, centered card, scoped CSS.

- [ ] **Step 2: Modify `webui/desktop/main/index.ts`** — In `CoreProcessManager`:
  - Remove `scheduleRestart()`, `restartDelay`, `restartTimer`, `CORE_RESTART_DELAY_MS`, `CORE_MAX_RESTART_DELAY_MS`
  - In `process.on('exit', ...)`: replace restart logic with pushing `core-crashed` event to `eventListeners`
  - Add public `async restart(): Promise<void>` method
  - In `createWindow()`, register listener that forwards `core-crashed` to renderer

- [ ] **Step 3: Modify `webui/src/App.vue`** — Add `showCoreCrashed`/`crashExitCode` refs. Listen for `core-crashed` IPC event. Conditionally render `<CoreCrashed>`. Wire restart/quit emits.

- [ ] **Step 4: Verify** — `cd webui && pnpm run build`. Launch Electron, kill core. Expected: CoreCrashed overlay. Test both buttons.

- [ ] **Step 5: Commit** — `git add` new/modified files, commit `"feat: add CoreCrashed UI overlay with manual restart/quit"`.

---

### Task 1D: Single-Instance Enforcement

**Files:** Modify `src/core/core_process.cpp`.

**Dependencies:** P1B (uses same pipe for guard)

- [ ] **Step 1: Add guard in `core_process_main()`** — The pipe listener from P1B already fails if pipe is bound. Add explicit error: `if (!pipe_listener->start()) { std::cerr << "fatal: another core process is already running" << std::endl; return 1; }`.

- [ ] **Step 2: Verify** — Start first `exv --mode=core` → succeeds. Start second → exits with error. Kill first → new one succeeds.

- [ ] **Step 3: Commit** — `git add src/core/core_process.cpp`, commit `"feat: enforce single-instance for core process via pipe exclusivity"`.

---

## Phase 2: Consumer Removal (2 parallel, depend on P1A)

### Task 2A: Remove Log-Tail Consumers + SSE Broadcaster

**Files:** Delete `src/sse_broadcaster.cpp`, `src/sse_broadcaster.hpp`. Modify `src/vpn.cpp`, `src/tunnel.cpp`, `src/tunnel.hpp`.

**Dependencies:** P1A

- [ ] **Step 1: Delete SSE broadcaster files** — `git rm src/sse_broadcaster.cpp src/sse_broadcaster.hpp`.

- [ ] **Step 2: Delete AuthFailureWatch and RuntimeLogTail from `src/vpn.cpp`** — Read `src/vpn.cpp`. Delete `RuntimeLogTail` class (lines ~54-98), `AuthFailureWatch` class (lines ~122-145), and all usages in `run_supervisor()`.

- [ ] **Step 3: Delete `runtime_log_has_auth_failure()`** — From `src/tunnel.cpp` (lines ~206-212) and `src/tunnel.hpp` (declaration).

- [ ] **Step 4: Build and verify** — `cmake --build build --target exv`. No references to deleted symbols.

- [ ] **Step 5: Commit** — `git add -u`, commit `"refactor: remove log-tail consumers and SSE broadcaster"`.

---

### Task 2B: Remove Supervisor from vpn.cpp + vpn_runtime.cpp

**Files:** Modify `src/vpn.cpp`, `src/vpn.hpp`, `src/vpn_runtime.cpp`.

**Dependencies:** P1A

- [ ] **Step 1: Delete supervisor functions from `src/vpn.cpp`** — Delete: `ConnectionDiagnostics`, `LoggingEventSink`, `write_pid_file`, `read_pid_file`, `remove_pid_file`, `write_pid`, `read_pid`, `remove_pid`, `write_supervisor_pid`, `read_supervisor_pid`, `remove_supervisor_pid`, `remove_route_ready`, `read_route_ready`, `clear_runtime_state`, `is_process_alive`, `find_openconnect_pid`, `terminate_process`, `sleep_ms`, `append_openconnect_attempt_marker`, `handle_supervisor_signal`, `describe_retry_policy`, `run_native_supervisor()`, `run_supervisor()`, `supervisor_main()`. In `start()`: remove old supervisor branches, keep only TunnelController path.

- [ ] **Step 2: Update `src/vpn.hpp`** — Remove declarations for all deleted functions.

- [ ] **Step 3: Rewrite `src/vpn_runtime.cpp`** — Delete local `read_pid_file()`, `read_route_ready()`, `is_process_alive()`, `find_openconnect_pid()`. Rewrite `read_runtime_status_snapshot()` to remove all file-based paths; return `running = false` if core not reachable.

- [ ] **Step 4: Build and verify** — `cmake --build build --target exv`. No supervisor functions. No PID/route-ready references in vpn.cpp.

- [ ] **Step 5: Commit** — `git add src/vpn.cpp src/vpn.hpp src/vpn_runtime.cpp`, commit `"refactor: remove legacy supervisor path"`.

---

## Phase 3: Deep Cleanup (3 parallel, depend on P2A+P2B)

### Task 3A: Remove native_session_store Module

**Files:** Delete `src/vpn_engine/native_session_store.cpp`, `src/vpn_engine/native_session_store.hpp`. Modify `src/vpn.cpp`, `src/helper.cpp`, `src/app_api.cpp`, `src/vpn_runtime.cpp`. Move preserved data structures to `src/vpn_engine/session_state.hpp`.

**Dependencies:** P2A + P2B

- [ ] **Step 1: Delete the two files** — `git rm src/vpn_engine/native_session_store.cpp src/vpn_engine/native_session_store.hpp`.

- [ ] **Step 2: Remove `#include` from consumers** — In `src/vpn.cpp`, `src/helper.cpp`, `src/app_api.cpp`, `src/vpn_runtime.cpp`: remove `#include "vpn_engine/native_session_store.hpp"`.

- [ ] **Step 3: Delete `cleanup_legacy_supervisor_state_files()` from `src/app_api.cpp`** — This function called `clear_native_session_state()` and removed supervisor PID file. No longer needed.

- [ ] **Step 4: Preserve data structures** — Ensure `SessionPhase`, `SessionState`, `TunnelMetadata` are defined in `src/vpn_engine/session_state.hpp` (they already are — verify).

- [ ] **Step 5: Build and verify** — `cmake --build build --target exv`. No `#include "vpn_engine/native_session_store.hpp"` anywhere.

- [ ] **Step 6: Commit** — `git add -u`, commit `"refactor: remove native_session_store module"`.

---

### Task 3B: Remove PID/Route-Ready Path Utilities

**Files:** Modify `src/utils.cpp`, `src/utils.hpp`, `src/platform/common/path_utils.hpp`, `src/platform/{darwin,linux,win32}/path_utils.cpp`.

**Dependencies:** P2A + P2B

- [ ] **Step 1: Delete path getters from `src/utils.cpp`** — Remove `get_pid_path()`, `get_supervisor_pid_path()`, `get_route_ready_path()`. Keep `get_tunnel_path()` if still needed for tunnel script generation.

- [ ] **Step 2: Delete declarations from `src/utils.hpp`** — Remove corresponding declarations.

- [ ] **Step 3: Delete from `src/platform/common/path_utils.hpp`** — Remove `pid_path()`, `supervisor_pid_path()`, `route_ready_path()`.

- [ ] **Step 4: Delete from platform impls** — Remove implementations from `src/platform/darwin/path_utils.cpp`, `src/platform/linux/path_utils.cpp`, `src/platform/win32/path_utils.cpp`.

- [ ] **Step 5: Build and verify** — `cmake --build build --target exv`. No references to deleted path getters.

- [ ] **Step 6: Commit** — `git add -u`, commit `"refactor: remove PID/route-ready path utilities"`.

---

### Task 3C: Clean Up Helper Daemon V1 File-Based State

**Files:** Modify `src/helper.cpp`, `src/helper.hpp`.

**Dependencies:** P2A + P2B

- [ ] **Step 1: Delete V1 functions from `src/helper.cpp`** — Remove: `pid_path_for()`, `supervisor_pid_path_for()`, `route_ready_path_for()`, `clear_runtime_state()`, `read_pid_file()`, `read_route_ready()`, `is_process_alive()`, `find_openconnect_pid()`, `inspect_runtime()`, `save_session_state()`, `load_session_state()`, `clear_session_state()`, `clear_native_session_state_for_known_config_dirs()`, `stop_managed_session()`, `handle_start()`, `handle_stop()`, `handle_status()`, `make_status_response()`, `print_running_status()`, `create_request_file()`.

- [ ] **Step 2: Remove V1 dispatch from `handle_request()`** — Delete start/stop/status action branches. Keep only V2 dispatch.

- [ ] **Step 3: Clean up `daemon_main()` accept loop** — Remove V1 request handling. Keep V2 handler dispatch (`helper_v2_handler.hpp`).

- [ ] **Step 4: Update `src/helper.hpp`** — Remove `start_via_helper()`, `stop_via_helper()`, `status_via_helper()` if present. Keep `is_available()`, `daemon_main()`, `worker_main()`, `install_service()`, `uninstall_service()`.

- [ ] **Step 5: Build and verify** — `cmake --build build --target exv-helper`. Helper daemon starts and responds to V2 Hello/StartSession. No references to PID files, route-ready, native-session-state.json in helper.cpp.

- [ ] **Step 6: Commit** — `git add src/helper.cpp src/helper.hpp`, commit `"refactor: remove V1 file-based state from helper daemon"`.

---

## Phase 4: CLI Thin-Layer Refactoring + WebUI Removal

**Files:** Delete `src/webui.cpp`, `src/webui.hpp`. Create `src/cli/pipe_client.hpp`, `src/cli/pipe_client.cpp`. Modify `src/main.cpp`.

**Dependencies:** P1B, P3A, P3B, P3C

- [ ] **Step 1: Delete WebUI files** — `git rm src/webui.cpp src/webui.hpp`.

- [ ] **Step 2: Create `src/cli/pipe_client.hpp`** — `PipeClient` class: `connect(pipe_path)` → `send_request(json_line)` → `recv_response()` → `disconnect()`. Thin wrapper over platform pipe/socket.

- [ ] **Step 3: Create `src/cli/pipe_client.cpp
** — Cross-platform implementation: Windows `CreateFileA` + `WriteFile`/`ReadFile`, Unix `socket(AF_UNIX)` + `connect`/`write`/`read`.

- [ ] **Step 4: Rewrite `src/main.cpp` CLI command handlers** — Each CLI command (start/stop/status/config/logs/service) now:
  1. Creates a `PipeClient`, connects to `core_pipe_path()`
  2. If connect fails: spawn `exv --mode=core` as detached child, poll pipe up to 5s, retry connect
  3. Sends JSON-RPC request, receives response, formats output, exits
  4. Remove `desktop-rpc`/`desktop-rpc-file`/`desktop-rpc-file-output` entry points
  5. Remove `__helper-daemon`, `__tunnel-script`, `__helper-exec`, `__vpn-supervisor` entry points
  6. Remove all WebUI foreground/background mode logic (`--foreground`, `-f`, fork/daemonize)

- [ ] **Step 5: Build and verify** — `cmake --build build --target exv`. `exv status` works (auto-spawns core if needed). `exv start` sends connect and exits immediately. `exv logs` shows log lines.

- [ ] **Step 6: Commit** — `git add -u src/webui.cpp src/webui.hpp src/cli/pipe_client.hpp src/cli/pipe_client.cpp src/main.cpp`, commit `refactor: CLI thin-layer + remove WebUI`.

---

## Phase 5: Integration & Final Cleanup

**Files:** Modify `CMakeLists.txt`, `src/main.cpp`, `src/config.hpp`, `src/config.cpp`.

**Dependencies:** P1B, P1C, P2B, P3A, P3C, P4

- [ ] **Step 1: Update `CMakeLists.txt`** — Remove deleted source files from build targets: `native_session_store.cpp`, `sse_broadcaster.cpp`, `webui.cpp`. Add new files: `log_event_bus.cpp`, `log_renderer.cpp`, `pipe_ipc.cpp`, `cli/pipe_client.cpp`. Remove `embed_assets` custom command and `add_dependencies(exv embed_assets)`. Remove `src/webui_assets.hpp` custom command. Remove or update test targets that reference deleted files (`native_helper_session_test`, `vpn_runtime_test`).

- [ ] **Step 2: Remove WebUI config fields from `src/config.hpp`** — Delete `webui_port`, `webui_bind`, `webui_enabled` from `Config` struct. Update `NLOHMANN_DEFINE_TYPE_INTRUSIVE_WITH_DEFAULT` macro to remove these fields.

- [ ] **Step 3: Update `src/config.cpp`** — Remove any references to WebUI config fields in serialization/defaults.

- [ ] **Step 4: Update platform config defaults** — Remove `webui_enabled` from `src/platform/darwin/config_defaults.cpp`, `src/platform/win32/config_defaults.cpp`, `src/platform/common/config_defaults_linux.cpp`.

- [ ] **Step 5: Audit `#include` directives** — Search for and remove any remaining `#include` references to deleted headers across all source files.

- [ ] **Step 6: Cross-platform build** — `cmake --build build --target exv` and `cmake --build build --target exv-helper` on the current platform. Verify clean build with no warnings about missing files.

- [ ] **Step 7: Run test suite** — `cd build && ctest --output-on-failure`. Update or remove any tests that reference deleted functions. Expected: all remaining tests pass.

- [ ] **Step 8: Verify `exv --help`** — Run `./build/exv --help`. Expected: shows only CLI commands and `--mode=core`. No WebUI options.

- [ ] **Step 9: Commit** — `git add -u CMakeLists.txt src/config.hpp src/config.cpp src/main.cpp src/platform/*/config_defaults.cpp`, commit `chore: integration cleanup, remove dead includes and WebUI config`.

---

## Cross-Reference: Conflict Resolution

| Conflict | Files | Resolution |
|----------|-------|------------|
| P2A + P2B both edit `src/vpn.cpp` | `src/vpn.cpp` | Execute P2A first (removes AuthFailureWatch/RuntimeLogTail), then P2B (removes supervisor). Or merge into single edit pass. |
| P2B + P3A both touch `src/vpn_runtime.cpp` | `src/vpn_runtime.cpp` | P2B rewrites status snapshot, P3A only removes an include. Execute sequentially. |
| P3A + P3C both touch `src/helper.cpp` | `src/helper.cpp` | P3A removes include, P3C deletes functions. Execute sequentially. |

## Deleted Files Summary

| File | Phase | Reason |
|------|-------|--------|
| `src/vpn_engine/native_session_store.cpp` | P3A | File-based state persistence |
| `src/vpn_engine/native_session_store.hpp` | P3A | File-based state persistence |
| `src/sse_broadcaster.cpp` | P2A | WebUI-only; Electron uses JSON-RPC push |
| `src/sse_broadcaster.hpp` | P2A | WebUI-only |
| `src/webui.cpp` | P4 | WebUI retired; Electron is sole GUI |
| `src/webui.hpp` | P4 | WebUI retired |

## New Files Summary

| File | Phase | Purpose |
|------|-------|---------|
| `src/log_event_bus.hpp` | P1A | Typed in-process event bus |
| `src/log_event_bus.cpp` | P1A | Implementation |
| `src/log_renderer.hpp` | P1A | TypedEvent → text bridge |
| `src/log_renderer.cpp` | P1A | Implementation |
| `src/core/pipe_ipc.hpp` | P1B | Cross-platform pipe listener |
| `src/core/pipe_ipc.cpp` | P1B | Implementation |
| `src/cli/pipe_client.hpp` | P4 | CLI pipe client wrapper |
| `src/cli/pipe_client.cpp` | P4 | Implementation |
| `webui/src/components/CoreCrashed.vue` | P1C | Core crash overlay |
