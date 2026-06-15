# Base And Observability Foundation Design

## Summary

This design adopts the revised direction: do not create a large mandatory `base` layer that every module must depend on. Instead, split the current `types`, `errors`, and `log` concerns into small foundation libraries with clear dependency rules:

- `src/base`: pure, side-effect-free shared models and error primitives.
- `src/observability`: logging API, log event model, async queue, worker-owned dispatch, and sink interfaces.
- `src/platform/*/logging`: concrete file and IPC/RPC log sink or transport implementations.

The goal is to let `core`, `helper`, `vpn_engine`, `platform`, `runtime`, `app`, and future modules log without depending on `core`, while preventing `base` from becoming a new dumping ground.

## Current State

The repository currently has these relevant pieces:

- Logging API and event bus live in `src/common/diagnostics/logger.*`, `log_event_bus.*`, and `log_renderer.*`.
- `logger.cpp` depends directly on platform runtime paths and file APIs.
- `LogEventBus::publish()` synchronously invokes subscribers while holding a mutex.
- `LogRenderer` subscribes to the bus and writes text lines to disk.
- Error types are scattered:
  - `src/common/errors/error_types.hpp`
  - `src/feedback/error_contract.*`
  - `src/core/tunnel_controller/core_error_mapper.*`
  - `src/vpn_engine/native_error_contract.hpp`
  - `src/helper/common/helper_error.hpp`
- Existing C++20 module pilots already exist for helper, config, tunnel, and utils, so this design should preserve that pattern without forcing platform into modules first.

## Adopted Architecture

```text
src/base
  types/
  errors/
  modules/

src/observability
  log_event.hpp
  log_level.hpp
  log_queue.hpp
  log_sink.hpp
  log_service.hpp
  log_facade.hpp
  modules/

src/platform/common/logging
  file_log_sink.hpp/.cpp
  ipc_log_sink.hpp/.cpp
  log_runtime_paths.hpp/.cpp

src/common/diagnostics
  compatibility wrappers during migration
```

Dependency direction:

```text
base
  ^
observability
  ^
app / core / helper / vpn_engine / runtime / platform

platform/logging -> observability interfaces
app/helper/core process startup -> assembles LogService with platform sinks
```

`base` must not depend on `observability`, `platform`, `core`, `helper`, `vpn_engine`, `runtime`, `app`, `cli`, or `nlohmann/json` unless a later explicit plan moves a pure serialization boundary there. First pass keeps JSON out of `base`.

`observability` may depend on `base`, the C++ standard library, and no platform-specific headers. It owns the log queue and worker thread, but not platform paths, file permissions, named pipes, or sockets.

`platform` implements sinks and transports that know about files, runtime directories, owner sync, named pipe/socket endpoints, and OS-specific IPC.

## Why Not Make Everything Depend On Base

A mandatory all-module `base` would make `base` grow into a catch-all. The safer rule is:

- Put a type in `base` only when at least two independent modules need to exchange it and it has no runtime side effects.
- Keep module-owned domain types in their module when they are not a cross-module contract.
- Put logging execution in `observability`, not `base`, because worker threads, queues, flush, shutdown, and sinks are behavior, not pure types.

## Base Scope

Initial `base` scope:

- `base/types/result.hpp`
  - `Result<T>` or non-template `Status` depending on implementation feasibility.
  - `Status` with string error codes and helpers for success/failure.
- `base/errors/error_info.hpp`
  - canonical `ErrorInfo` fields currently duplicated in feedback and tunnel state.
  - `ErrorDomain` and stable string constants for transport/auth/helper/platform/config/packet.
- `base/types/source_location.hpp`
  - small wrapper for source file, line, function, when needed by logs.

Out of scope for the first migration:

- Moving every platform-specific enum into `base`.
- Moving all helper protocol messages into `base`.
- Moving JSON serialization into `base`.
- Making `platform` a C++20 module.

## Observability Scope

Initial `observability` scope:

- `LogLevel`: `Trace`, `Debug`, `Info`, `Warn`, `Error`, `Fatal`.
- `LogEvent`: timestamp, level, component, code, message, fields, thread id, process role.
- `LogSink`: interface with `write(const LogEvent&)`, `flush()`, and `shutdown()`.
- `LogService`: owns a bounded message queue and one worker thread.
- `LogFacade`: process-wide facade used by existing call sites.
- Compatibility API: existing `ecnuvpn::logger::{init,info,warn,error,event,tail,show_logs}` remains available during migration.

Queue behavior:

- Producers never write files or IPC directly.
- Producers enqueue events with minimal locking.
- A single worker thread drains the queue and calls registered sinks.
- Shutdown drains queued events before stopping unless forced by process termination.
- Queue overflow policy is explicit: drop oldest non-error events first, always try to keep `Error` and `Fatal`.

IPC/RPC behavior:

- Internal modules publish log events to `LogService`.
- The worker may fan out to multiple sinks:
  - file sink,
  - stdout/event stream sink for core process,
  - helper/core IPC sink when configured.
- IPC/RPC transport is implemented outside `observability`, under `platform`, and registered as a sink.

## Compatibility Strategy

Existing code includes `common/diagnostics/logger.hpp` widely. A big-bang include migration would be noisy and risky. The first implementation preserves those headers as wrappers over `observability`.

Migration order:

1. Introduce `base` and `observability` without changing call sites.
2. Reimplement `common/diagnostics/logger.*`, `log_event_bus.*`, and `log_renderer.*` as compatibility shims.
3. Add architecture tests that block new direct dependencies on old internals.
4. Migrate module call sites gradually to `observability/log_facade.hpp`.
5. Delete old compatibility shims only after all production includes move.

## C++20 Module Strategy

Add narrow named modules after headers are stable:

- `exv.base.types`
- `exv.base.errors`
- `exv.observability.log`

The module files export only stable facade types and functions. They should not export platform sinks. Header APIs remain the source of truth until all supported compilers and tests are stable.

## Test Strategy

Add tests before implementation:

- Source architecture test:
  - `src/base` does not include `platform`, `core`, `helper`, `vpn_engine`, `runtime`, `app`, or `cli`.
  - `src/observability` does not include `platform`, `core`, `helper`, `vpn_engine`, `runtime`, `app`, or `cli`.
  - `src/common/diagnostics` is a compatibility layer only after migration starts.
- Unit tests:
  - queue drains all events on shutdown,
  - sink failures do not block other sinks,
  - overflow policy preserves high-severity logs,
  - facade can be used before explicit process setup,
  - `tail()` still reads the configured file sink output.
- Integration tests:
  - core process still emits log events over stdout,
  - desktop `logs.list` still returns recent log lines,
  - helper and platform code can log without depending on `core`.

## Success Criteria

- `core` is no longer required for logging.
- New `base` contains only pure shared types and errors.
- New `observability` owns async logging behavior and message queue.
- Platform-specific log file and IPC details are implemented in `platform`.
- Existing public logging behavior remains compatible during migration.
- Architecture tests prevent `base` and `observability` from growing upward dependencies.
