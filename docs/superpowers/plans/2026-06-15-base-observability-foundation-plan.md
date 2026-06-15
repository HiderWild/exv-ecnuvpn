# Base And Observability Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Split shared types, errors, and logging into a small pure `base` layer plus an async `observability` layer, so modules can log without going through `core` and without turning `base` into a catch-all.

**Architecture:** `base` owns pure shared result/error/type models. `observability` owns log events, queue, service, facade, and sink interfaces. `platform` owns file and IPC/RPC sink implementations, while `common/diagnostics` remains a temporary compatibility facade during migration.

**Tech Stack:** C++20, CMake 3.28, CTest, standard library threading primitives, existing platform path/file APIs, existing C++20 named module smoke-test pattern.

---

## File Structure

### New Base Layer

- `src/base/types/status.hpp`
  - `exv::base::Status`
  - pure success/failure helpers
- `src/base/errors/error_info.hpp`
  - `exv::base::ErrorInfo`
  - canonical error domain/code constants
- `src/base/modules/types.cppm`
  - named module `exv.base.types`
- `src/base/modules/errors.cppm`
  - named module `exv.base.errors`

### New Observability Layer

- `src/observability/log_level.hpp`
  - log severity enum and string conversion
- `src/observability/log_event.hpp`
  - structured log event model
- `src/observability/log_sink.hpp`
  - sink interface
- `src/observability/log_queue.hpp/.cpp`
  - bounded thread-safe queue and overflow policy
- `src/observability/log_service.hpp/.cpp`
  - worker thread, sink registration, enqueue, flush, shutdown
- `src/observability/log_facade.hpp/.cpp`
  - process-wide logging facade
- `src/observability/modules/log.cppm`
  - named module `exv.observability.log`

### Platform Logging Implementations

- `src/platform/common/logging/file_log_sink.hpp/.cpp`
  - writes rendered events to configured log path
- `src/platform/common/logging/stdout_log_sink.hpp/.cpp`
  - emits JSON log events to stdout for core process mode
- `src/platform/common/logging/log_runtime.hpp/.cpp`
  - configures default process logging using platform runtime paths

### Compatibility Layer

- Modify `src/common/diagnostics/logger.hpp/.cpp`
  - keep existing API, delegate to `observability::LogFacade`
- Modify `src/common/diagnostics/log_event_bus.hpp/.cpp`
  - either delegate to `LogService` subscriptions or become a thin adapter
- Modify `src/common/diagnostics/log_renderer.hpp/.cpp`
  - remove direct file writing responsibility after file sink exists

### Tests

- Create `tests/base_observability_architecture_test.cpp`
- Create `tests/log_queue_test.cpp`
- Create `tests/log_service_test.cpp`
- Create `tests/base_module_smoke_test.cpp`
- Create `tests/observability_module_smoke_test.cpp`
- Modify `tests/app_api_logs_test.cpp`
- Modify `tests/core_process_lifecycle_test.cpp`
- Modify `tests/security/no_secret_in_logs_test.cpp`

---

## Phase 0: Architecture Gates

**Files:**
- Create: `tests/base_observability_architecture_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] Add a read-only architecture test.

The test scans source files and verifies:

```cpp
// Required checks:
// 1. src/base exists after Phase 1.
// 2. src/observability exists after Phase 2.
// 3. src/base does not include platform/core/helper/vpn_engine/runtime/app/cli.
// 4. src/observability does not include platform/core/helper/vpn_engine/runtime/app/cli.
// 5. src/platform may include observability interfaces.
// 6. src/common/diagnostics may include observability only as compatibility.
```

Use this helper pattern:

```cpp
bool tree_contains_forbidden_include(const std::filesystem::path& root,
                                     const std::vector<std::string>& needles);
```

- [ ] Run the test and confirm the initial version passes with existence checks disabled until the target folders are created.

```powershell
cmake --build build --target base_observability_architecture_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^base_observability_architecture_test$"
```

- [ ] Commit Phase 0.

```powershell
git add CMakeLists.txt tests/base_observability_architecture_test.cpp
git commit -m "test: add base observability architecture gates"
```

---

## Phase 1: Add Pure Base Types And Errors

**Files:**
- Create: `src/base/types/status.hpp`
- Create: `src/base/errors/error_info.hpp`
- Create: `src/base/modules/types.cppm`
- Create: `src/base/modules/errors.cppm`
- Create: `tests/base_module_smoke_test.cpp`
- Modify: `CMakeLists.txt`
- Modify: `tests/base_observability_architecture_test.cpp`

- [ ] Add failing tests for base purity and module exports.

`tests/base_module_smoke_test.cpp` should import both modules:

```cpp
import exv.base.types;
import exv.base.errors;

#include <iostream>
#include <string>

int main() {
  bool ok = true;
  auto status = exv::base::Status::success_status();
  ok = ok && status.ok();

  auto failure = exv::base::Status::failure("config_invalid",
                                            "Config is invalid");
  ok = ok && !failure.ok();
  ok = ok && failure.code == "config_invalid";

  exv::base::ErrorInfo error;
  error.domain = exv::base::error_domains::Config;
  error.code = exv::base::error_codes::InvalidConfig;
  error.message = "Config is invalid";
  ok = ok && error.domain == "config";

  if (!ok) {
    std::cerr << "base_module_smoke_test failed\n";
    return 1;
  }
  std::cout << "base_module_smoke_test passed\n";
  return 0;
}
```

- [ ] Implement `Status`.

`src/base/types/status.hpp`:

```cpp
#pragma once

#include <string>
#include <utility>

namespace exv::base {

struct Status {
  bool success = true;
  std::string code;
  std::string message;

  bool ok() const { return success; }

  static Status success_status() { return Status{}; }

  static Status failure(std::string code, std::string message) {
    Status status;
    status.success = false;
    status.code = std::move(code);
    status.message = std::move(message);
    return status;
  }
};

} // namespace exv::base
```

- [ ] Implement `ErrorInfo`.

`src/base/errors/error_info.hpp`:

```cpp
#pragma once

#include <optional>
#include <string>

namespace exv::base {

namespace error_domains {
inline constexpr const char* Transport = "transport";
inline constexpr const char* Auth = "auth";
inline constexpr const char* Helper = "helper";
inline constexpr const char* Platform = "platform";
inline constexpr const char* Config = "config";
inline constexpr const char* Packet = "packet";
} // namespace error_domains

namespace error_codes {
inline constexpr const char* InvalidConfig = "config_invalid";
inline constexpr const char* UnknownAction = "unknown_action";
inline constexpr const char* HelperUnavailable = "helper_unavailable";
inline constexpr const char* TransportClosed = "transport_closed";
inline constexpr const char* AuthFailed = "auth_failed";
} // namespace error_codes

struct ErrorInfo {
  std::string domain;
  std::string code;
  std::string message;
  std::optional<int> native_code;
  std::string native_api;
  bool recoverable = false;
  std::string recommended_action;
};

} // namespace exv::base
```

- [ ] Add named module wrappers.

`src/base/modules/types.cppm`:

```cpp
export module exv.base.types;

export import <string>;
export import <utility>;

export namespace exv::base {
struct Status {
  bool success = true;
  std::string code;
  std::string message;

  bool ok() const { return success; }
  static Status success_status() { return Status{}; }
  static Status failure(std::string code, std::string message) {
    Status status;
    status.success = false;
    status.code = std::move(code);
    status.message = std::move(message);
    return status;
  }
};
}
```

`src/base/modules/errors.cppm`:

```cpp
export module exv.base.errors;

export import <optional>;
export import <string>;

export namespace exv::base {
namespace error_domains {
inline constexpr const char* Transport = "transport";
inline constexpr const char* Auth = "auth";
inline constexpr const char* Helper = "helper";
inline constexpr const char* Platform = "platform";
inline constexpr const char* Config = "config";
inline constexpr const char* Packet = "packet";
}

namespace error_codes {
inline constexpr const char* InvalidConfig = "config_invalid";
inline constexpr const char* UnknownAction = "unknown_action";
inline constexpr const char* HelperUnavailable = "helper_unavailable";
inline constexpr const char* TransportClosed = "transport_closed";
inline constexpr const char* AuthFailed = "auth_failed";
}

struct ErrorInfo {
  std::string domain;
  std::string code;
  std::string message;
  std::optional<int> native_code;
  std::string native_api;
  bool recoverable = false;
  std::string recommended_action;
};
}
```

- [ ] Add CMake targets for `exv-base-types-module`, `exv-base-errors-module`, and `base_module_smoke_test`.

Follow the existing module target pattern used by `exv-core-tunnel-errors-module`.

- [ ] Strengthen architecture gates so `src/base` may not include upward modules.

- [ ] Run tests.

```powershell
cmake --build build --target base_module_smoke_test base_observability_architecture_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(base_module_smoke_test|base_observability_architecture_test)$"
```

- [ ] Commit Phase 1.

```powershell
git add CMakeLists.txt src/base tests/base_module_smoke_test.cpp tests/base_observability_architecture_test.cpp
git commit -m "feat: add base status and error primitives"
```

---

## Phase 2: Add Async Observability Core

**Files:**
- Create: `src/observability/log_level.hpp`
- Create: `src/observability/log_event.hpp`
- Create: `src/observability/log_sink.hpp`
- Create: `src/observability/log_queue.hpp`
- Create: `src/observability/log_queue.cpp`
- Create: `src/observability/log_service.hpp`
- Create: `src/observability/log_service.cpp`
- Create: `src/observability/log_facade.hpp`
- Create: `src/observability/log_facade.cpp`
- Create: `tests/log_queue_test.cpp`
- Create: `tests/log_service_test.cpp`
- Modify: `CMakeLists.txt`
- Modify: `tests/base_observability_architecture_test.cpp`

- [ ] Add `log_queue_test` first.

Required assertions:

```cpp
// 1. push() then pop() preserves FIFO order.
// 2. close() wakes waiters and causes pop() to return false when empty.
// 3. bounded queue overflow drops oldest Info events first.
// 4. Error events are retained ahead of Info events when overflow happens.
```

- [ ] Implement `LogLevel` and `LogEvent`.

`LogEvent` fields:

```cpp
std::chrono::system_clock::time_point timestamp;
LogLevel level;
std::string component;
std::string code;
std::string message;
std::vector<std::pair<std::string, std::string>> fields;
std::string process_role;
std::uint64_t thread_id_hash;
```

- [ ] Implement bounded `LogQueue`.

Interface:

```cpp
class LogQueue {
public:
  explicit LogQueue(std::size_t capacity);
  bool push(LogEvent event);
  bool pop(LogEvent& out);
  void close();
  std::size_t dropped_count() const;
};
```

Overflow policy:

- if full and new event is `Info` or below, drop the oldest event at `Info` or below;
- if full and no low-severity event exists, drop the new low-severity event;
- if full and new event is `Warn`, `Error`, or `Fatal`, drop the oldest low-severity event first;
- if the queue contains only high-severity events, drop the oldest event and increment dropped count.

- [ ] Add `log_service_test`.

Required assertions:

```cpp
// 1. start() launches one worker and stop() drains queued events.
// 2. multiple sinks receive the same event.
// 3. one throwing/failing sink does not stop later sinks.
// 4. flush() waits until all queued events before flush are delivered.
// 5. facade calls before explicit setup do not crash.
```

- [ ] Implement `LogSink`.

```cpp
class LogSink {
public:
  virtual ~LogSink() = default;
  virtual void write(const LogEvent& event) = 0;
  virtual void flush() {}
  virtual void shutdown() {}
};
```

- [ ] Implement `LogService`.

Interface:

```cpp
class LogService {
public:
  explicit LogService(std::size_t queue_capacity = 4096);
  ~LogService();

  void add_sink(std::shared_ptr<LogSink> sink);
  void start();
  void stop();
  void flush();
  bool submit(LogEvent event);
  std::size_t dropped_count() const;
};
```

Rules:

- `start()` is idempotent.
- `stop()` drains queue, calls `flush()` and `shutdown()` on sinks, then joins worker.
- sink exceptions are swallowed and counted internally; they do not stop the worker.

- [ ] Implement `LogFacade`.

Compatibility-facing functions:

```cpp
void configure(std::shared_ptr<LogService> service);
LogService& default_service();
void info(std::string message);
void warn(std::string message);
void error(std::string message);
void event(LogLevel level, std::string component, std::string code,
           std::string message,
           std::vector<std::pair<std::string, std::string>> fields = {});
void flush();
void shutdown();
```

- [ ] Add CMake targets and link `exv-core` to observability sources.

- [ ] Strengthen architecture gates for `src/observability`.

- [ ] Run tests.

```powershell
cmake --build build --target log_queue_test log_service_test base_observability_architecture_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(log_queue_test|log_service_test|base_observability_architecture_test)$"
```

- [ ] Commit Phase 2.

```powershell
git add CMakeLists.txt src/observability tests/log_queue_test.cpp tests/log_service_test.cpp tests/base_observability_architecture_test.cpp
git commit -m "feat: add async observability log service"
```

---

## Phase 3: Platform Log Sinks

**Files:**
- Create: `src/platform/common/logging/file_log_sink.hpp`
- Create: `src/platform/common/logging/file_log_sink.cpp`
- Create: `src/platform/common/logging/stdout_log_sink.hpp`
- Create: `src/platform/common/logging/stdout_log_sink.cpp`
- Create: `src/platform/common/logging/log_runtime.hpp`
- Create: `src/platform/common/logging/log_runtime.cpp`
- Create: `tests/platform_log_sink_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] Add `platform_log_sink_test`.

Required assertions:

```cpp
// 1. FileLogSink writes rendered lines to a supplied test path.
// 2. FileLogSink flushes data before the test reads it.
// 3. StdoutLogSink renders JSON with event, data, level, message, component, code, and fields.
// 4. FileLogSink redacts no fields by itself; redaction remains producer/test policy for this phase.
```

- [ ] Implement `FileLogSink`.

Constructor:

```cpp
explicit FileLogSink(std::string log_path,
                     std::function<bool(const std::string&)> sync_owner = {});
```

Behavior:

- append one rendered line per event,
- create parent directory if needed through existing platform file API or filesystem,
- call `sync_owner(log_path)` when provided,
- never throw from `write()`.

- [ ] Implement `StdoutLogSink`.

Constructor:

```cpp
explicit StdoutLogSink(std::ostream& out);
```

Behavior:

- writes one JSON object per line,
- uses the existing core process event shape:

```json
{"event":"log","data":{"level":"INFO","message":"...","component":"...","code":"...","fields":{}}}
```

- [ ] Implement `log_runtime`.

Functions:

```cpp
std::shared_ptr<observability::LogService>
create_default_log_service(bool emit_stdout_events);

void configure_default_logging(bool emit_stdout_events);
void shutdown_default_logging();
```

`create_default_log_service()` registers:

- `FileLogSink(platform::get_log_path(), platform::sync_owner)`;
- `StdoutLogSink(std::cout)` only when `emit_stdout_events == true`.

- [ ] Run tests.

```powershell
cmake --build build --target platform_log_sink_test log_service_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(platform_log_sink_test|log_service_test)$"
```

- [ ] Commit Phase 3.

```powershell
git add CMakeLists.txt src/platform/common/logging tests/platform_log_sink_test.cpp
git commit -m "feat: add platform logging sinks"
```

---

## Phase 4: Compatibility Facade Migration

**Files:**
- Modify: `src/common/diagnostics/logger.hpp`
- Modify: `src/common/diagnostics/logger.cpp`
- Modify: `src/common/diagnostics/log_event_bus.hpp`
- Modify: `src/common/diagnostics/log_event_bus.cpp`
- Modify: `src/common/diagnostics/log_renderer.hpp`
- Modify: `src/common/diagnostics/log_renderer.cpp`
- Modify: `src/core/core_process.cpp`
- Modify: `src/core/app_api/desktop_log_actions.cpp`
- Modify: `tests/app_api_logs_test.cpp`
- Modify: `tests/core_process_lifecycle_test.cpp`
- Modify: `tests/security/no_secret_in_logs_test.cpp`

- [ ] Add regression expectations before changing implementation.

Required behavior:

```cpp
// logger::info/warn/error still compile and enqueue events.
// logger::event still preserves level/component/code/message/fields.
// logger::tail returns lines from the configured file sink.
// desktop logs.list returns recent log lines.
// core process stdout still emits {"event":"log","data":...}.
```

- [ ] Reimplement `logger.cpp` as facade.

Mapping:

```cpp
logger::info(msg)  -> observability::info(std::move(msg))
logger::warn(msg)  -> observability::warn(std::move(msg))
logger::error(msg) -> observability::error(std::move(msg))
logger::event(...) -> observability::event(...)
logger::init()     -> platform::logging::configure_default_logging(false)
logger::write(...) -> compatibility-only direct file sink call until LogRenderer is removed
logger::tail(n)    -> read file sink path through platform::get_log_path()
```

During this phase keep `logger::write()` available for compatibility, but mark it as internal in comments.

- [ ] Replace `LogRenderer` responsibility.

`LogRenderer` becomes an RAII compatibility object that calls:

```cpp
platform::logging::configure_default_logging(true);
```

on construction and flushes/shuts down the stdout sink path on destruction only when it owns the configuration.

- [ ] Adapt `LogEventBus`.

Keep `TypedLogEvent` and subscription API for old tests, but implement publish by forwarding to `LogService`. Subscriptions should be modeled as in-process sinks that call the subscriber.

- [ ] Update `core_process.cpp`.

Remove direct `LogEventBus` subscription when `StdoutLogSink` is active through default logging configuration. The core process should not maintain a second log event fanout path after this phase.

- [ ] Run focused regression.

```powershell
cmake --build build --target exv app_api_logs_test core_process_lifecycle_test no_secret_in_logs_test log_service_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(app_api_logs_test|core_process_lifecycle_test|no_secret_in_logs_test|log_service_test)$"
```

- [ ] Commit Phase 4.

```powershell
git add src/common/diagnostics src/core/core_process.cpp src/core/app_api/desktop_log_actions.cpp tests/app_api_logs_test.cpp tests/core_process_lifecycle_test.cpp tests/security/no_secret_in_logs_test.cpp
git commit -m "refactor: route diagnostics through observability service"
```

---

## Phase 5: Migrate Production Includes Gradually

**Files:**
- Modify production files currently including `common/diagnostics/logger.hpp`
- Modify `tests/base_observability_architecture_test.cpp`

- [ ] Add a source inventory step.

Run:

```powershell
rg -n "#include \"common/diagnostics/logger.hpp\"" src
```

Record the modules in the commit message body when migrating each batch.

- [ ] Migrate low-risk modules first.

Batch 1:

- `src/platform/common/*.cpp`
- `src/platform/{win32,linux,darwin}/*.cpp`

Replace:

```cpp
#include "common/diagnostics/logger.hpp"
```

with:

```cpp
#include "observability/log_facade.hpp"
```

and call the equivalent facade functions.

- [ ] Run platform-focused tests.

```powershell
cmake --build build --target backend_resolver_test platform_status_models_test app_api_runtime_policy_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(backend_resolver_test|platform_status_models_test|app_api_runtime_policy_test)$"
```

- [ ] Commit platform include migration.

```powershell
git add src/platform tests/base_observability_architecture_test.cpp
git commit -m "refactor: migrate platform logging to observability facade"
```

- [ ] Migrate helper next.

Batch 2:

- `src/helper/**/*.cpp`
- `src/helper/**/*.hpp` only when needed

Run:

```powershell
cmake --build build --target helper_contract_test helper_messages_connector_test helper_delegating_network_ops_test helper_network_ops_adapter_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(helper_contract_test|helper_messages_connector_test|helper_delegating_network_ops_test|helper_network_ops_adapter_test)$"
```

- [ ] Commit helper include migration.

```powershell
git add src/helper
git commit -m "refactor: migrate helper logging to observability facade"
```

- [ ] Migrate vpn_engine.

Batch 3:

- `src/vpn_engine/**/*.cpp`
- `src/vpn_engine/**/*.hpp` only when needed

Run:

```powershell
cmake --build build --target native_engine_contract_test native_event_sink_test native_tls_stream_contract_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(native_engine_contract_test|native_event_sink_test|native_tls_stream_contract_test)$"
```

- [ ] Commit vpn_engine include migration.

```powershell
git add src/vpn_engine
git commit -m "refactor: migrate vpn engine logging to observability facade"
```

- [ ] Migrate core/app/runtime last.

Batch 4:

- `src/core/**/*.cpp`
- `src/app/**/*.cpp`
- `src/runtime/**/*.cpp`

Run:

```powershell
cmake --build build --target exv core_process_lifecycle_test app_api_rpc_dispatcher_test tunnel_controller_integration_test --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(core_process_lifecycle_test|app_api_rpc_dispatcher_test|tunnel_controller_integration_test)$"
```

- [ ] Commit core/app/runtime include migration.

```powershell
git add src/core src/app src/runtime
git commit -m "refactor: migrate core runtime logging to observability facade"
```

---

## Phase 6: Module Facades And Final Boundary Enforcement

**Files:**
- Create: `src/observability/modules/log.cppm`
- Create: `tests/observability_module_smoke_test.cpp`
- Modify: `CMakeLists.txt`
- Modify: `tests/base_observability_architecture_test.cpp`

- [ ] Add `observability_module_smoke_test`.

Test:

```cpp
import exv.observability.log;

#include <iostream>

int main() {
  exv::observability::LogEvent event;
  event.level = exv::observability::LogLevel::Info;
  event.message = "module smoke";
  if (exv::observability::to_string(event.level) != "INFO") {
    std::cerr << "unexpected log level string\n";
    return 1;
  }
  std::cout << "observability_module_smoke_test passed\n";
  return 0;
}
```

- [ ] Implement `exv.observability.log`.

Export:

- `LogLevel`
- `to_string(LogLevel)`
- `LogEvent`
- `LogSink`

Do not export `LogService` until the header and lifetime semantics stabilize.

- [ ] Add final architecture gates.

Final required checks:

```text
src/base:
  no include of platform/core/helper/vpn_engine/runtime/app/cli/observability

src/observability:
  no include of platform/core/helper/vpn_engine/runtime/app/cli

src/platform:
  may include observability

src/common/diagnostics:
  no direct file writing except compatibility tail/show_logs

src/core:
  no direct dependency required for helper/platform/vpn_engine logging
```

- [ ] Run final focused regression.

```powershell
cmake --build build --config Debug
ctest --test-dir build -C Debug --output-on-failure -R "^(base_observability_architecture_test|base_module_smoke_test|observability_module_smoke_test|log_queue_test|log_service_test|platform_log_sink_test|app_api_logs_test|core_process_lifecycle_test|no_secret_in_logs_test|helper_contract_test|native_engine_contract_test|tunnel_controller_integration_test)$"
git diff --check
```

- [ ] Commit Phase 6.

```powershell
git add CMakeLists.txt src/observability/modules tests/observability_module_smoke_test.cpp tests/base_observability_architecture_test.cpp
git commit -m "test: enforce base observability boundaries"
```

---

## Completion Criteria

- `base` exists and contains only pure shared types/errors.
- `observability` owns async logging queue, worker thread, facade, and sink interfaces.
- platform owns concrete file/stdout/IPC logging implementations.
- existing `logger::*` API remains compatible until migration is complete.
- no module needs to depend on `core` to write logs.
- queue shutdown drains events.
- sink failures do not block other sinks.
- architecture tests enforce dependency direction.
- focused regression and `git diff --check` pass.

## Suggested Subagent Workstreams

- Agent A: Phase 0-1 base primitives and architecture gates.
- Agent B: Phase 2 async observability queue/service/facade.
- Agent C: Phase 3 platform sinks.
- Agent D: Phase 4 compatibility facade and core process log stream.
- Agent E: Phase 5 include migration batches.
- Agent F: Phase 6 modules and final enforcement.
