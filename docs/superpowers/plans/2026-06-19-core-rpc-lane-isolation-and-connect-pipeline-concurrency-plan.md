# Core RPC Lane Isolation And Connect Pipeline Concurrency Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make frontend-to-core RPC responsive under long VPN operations and shorten perceived connect time by combining bounded core RPC lane isolation with a cancellable parallel VPN connect pipeline.

**Architecture:** The WebView host becomes an async request bridge: renderer messages are accepted quickly, pure shell actions stay local, and core responses are posted back by request id. The core process becomes a front-door dispatcher plus a fixed set of single-worker business lanes, so a long VPN connection cannot block diagnostics, read-model, config, or platform status requests. VPN connect becomes a tracked job with a first-failure coordinator: backend/helper readiness, Windows runtime/platform readiness, and native protocol handshake run concurrently after quick local validation, then successful branches rendezvous before any privileged network-device/route/DNS mutation.

**Tech Stack:** C++20, CMake/Ninja, nlohmann/json, std::jthread/std::stop_token or std::thread/std::atomic cancellation fallback, std::future/std::condition_variable, existing `CoreRpcClient` transport abstraction, WebView2/WKWebView/WebKitGTK host bridges, Vue 3/Pinia/Vite host tests.

---

## Current Evidence

- `src/app/ui_shell/ui_shell_runtime.cpp` installs a message handler that calls `handle_host_request(...)` and `client.invoke(...)` synchronously inside the platform message callback.
- `src/app/ui_shell/core_rpc_client.cpp` writes one request line and then blocks in `read_line()` until the next non-event response, so it assumes one in-flight request per client.
- `src/core/core_process.cpp` reads a request, dispatches it, writes the response, then reads the next request. It also has `g_desktop_dispatch_mutex`, which serializes every legacy desktop action.
- `src/core/rpc/app_rpc_dispatcher.cpp` directly invokes the registered handler on the caller thread.
- `src/core/app_api/desktop_vpn_actions.cpp` keeps `vpn.connect` synchronous through preflight, platform checks, controller creation, helper connection, and `TunnelController::connect`.
- `src/core/app_api/desktop_log_actions.cpp` handles `logs.list` as a direct log read and does not require preflight; observed log blocking after connect is therefore caused by request serialization, not log-page preflight.
- The generated JS bridge in `src/platform/win32/ui_shell/webview2_host_win32.cpp` already tracks pending promises by numeric `id`, so the renderer can tolerate out-of-order native responses once the native host posts them.
- Windows `preflight_connect_platform_checks()` calls `driver_status_json()` and then checks Wintun/TAP readiness. `driver_status_json()` currently launches PowerShell `Get-CimInstance Win32_NetworkAdapter` twice, once for Wintun and once for TAP.
- macOS and Linux `preflight_connect_platform_checks()` return an empty object with no driver checks. This explains why the same preflight stage is effectively instant on macOS but can cost seconds on Windows.
- Windows backend resolution checks SCM service state, optionally probes the helper with `Hello`, and in one-shot mode starts `exv-helper.exe` then waits up to forty 100 ms pipe-availability probes. This is independent from the read-only driver probe and can run concurrently.
- `CoreSessionRunner::start()` explicitly documents that `session_->start()` runs auth and CSTP synchronously; `NativeVpnEngineSession::start()` authenticates, connects CSTP, then starts the packet loop. To safely parallelize with helper/platform readiness, the native engine must be split into a handshake phase that does not yet open a packet device or mutate OS network state.

## Windows Connect Latency Findings

Ranked explanation for Windows feeling slower than macOS:

1. High confidence: Windows platform checks are expensive because they spawn PowerShell and run CIM network-adapter enumeration twice. macOS has no platform-specific driver checks in this path.
2. High confidence: Windows helper readiness can add extra time because service status uses SCM APIs and availability probing, and one-shot helper startup waits for a named pipe to appear after elevation/process creation.
3. Medium confidence: Native auth/CSTP may be comparable across platforms for the same gateway, but current code serializes it behind Windows-only backend and platform readiness, so Windows pays those costs before the gateway handshake starts.
4. Unknown until measured: exact per-machine breakdown between PowerShell/CIM, service/helper startup, named-pipe connect, and AnyConnect gateway latency. The implementation must add per-branch timing so later tuning is evidence-based.

## Parallel Connect Pipeline Model

Keep the initial gate synchronous and cheap:

1. Load config and password.
2. Validate required fields and native config.
3. Acquire the connection-attempt lock.
4. Create a connect job id and cancellation source.

Then start three required branches concurrently:

| Branch | Work | May Mutate OS Network? | Cancel Behavior |
| --- | --- | --- | --- |
| `backend_helper_ready` | `resolve_backend`, one-shot helper startup if needed, helper pipe connection, helper `Hello`, core lease, helper `StartSession` | No route/DNS/device mutation; helper process/session state only | On cancellation, release lease and request helper shutdown/cleanup if a session was opened. |
| `platform_ready` | `runtime_status_json`, Windows driver status, Wintun/TAP readiness, cached/native adapter probe | No | Stop after current probe; late errors are logged with `discarded_due_to=<first_failure_code>`. |
| `protocol_handshake` | Native transport creation, auth, auth-interaction waiting, CSTP connect, metadata extraction | No packet device open, no route/DNS/device mutation | Call transport/session disconnect; late errors are logged only if non-cancel failure occurs. |

Rendezvous only after all three branches succeed. The serial tail remains:

1. Build/reuse `TunnelController`.
2. Attach the prepared helper session and prepared native handshake.
3. Prepare packet device.
4. Apply route/DNS/network config.
5. Start packet loop and mark connected.

First-failure rule:

- The first required branch to fail wins the user-visible error.
- The coordinator immediately marks the connect job failed, publishes status/error to UI, and requests cancellation of other branches.
- Branches that later return success are discarded silently.
- Branches that later return a non-cancel failure write one diagnostic log line with `job_id`, `branch`, `error_code`, and `discarded_due_to`.
- User-visible failure must not wait for late branch cleanup unless cleanup is required to prevent leaked OS network state.

## VPN User Intent And Cancel Model

The VPN workflow is heavy, so renderer clicks must update a core-owned desired intent instead of spawning or interrupting arbitrary work.

State model:

```cpp
enum class DesiredVpnIntent { Disconnect, Connect };

struct VpnWorkflowIntent {
  DesiredVpnIntent desired = DesiredVpnIntent::Disconnect;
  std::uint64_t epoch = 0;
  std::string profile_id;
  bool has_password = false;
};
```

Rules:

- `vpn.connect` while idle sets `desired=Connect`, increments `epoch`, stores the latest in-memory connect request for this process, starts one connect job, and returns `accepted=true`.
- `vpn.connect` while a connect/cancel/disconnect workflow is busy does not start a second heavy workflow. It updates `desired=Connect`, increments `epoch`, refreshes the pending in-memory connect request, and returns `accepted=true`, `coalesced=true`, and `active_job_id`.
- `vpn.disconnect` while a connect workflow is busy means "cancel the in-progress connect". It sets `desired=Disconnect`, increments `epoch`, requests cancellation on the active job, and returns `accepted=true`, `cancelling=true`, `user_cancelled=true`. This is not an error.
- User cancellation must not show an error modal and must not write a failure log. It may write a low-noise info/debug event such as `connect.cancel.requested` only when needed for diagnostics.
- Branch results produced after user cancellation are discarded. `session_cancelled` and equivalent cancel acknowledgements are silent. A late non-cancel cleanup failure may be logged only if it indicates leaked helper/session/network state.
- When a workflow becomes idle, a reconciler checks the latest intent. If `desired=Disconnect`, remain disconnected. If `desired=Connect` and the intent `epoch` is newer than the job that just finished or was cancelled, start exactly one new connect job with the newest pending connect request.
- A non-user failure must not cause an infinite reconnect loop. If the failed job already represented the newest `Connect` intent, stay failed/disconnected and surface the error. Only a later user click that increments `epoch` can request another connect.
- Auto-reconnect policy remains separate and applies only after a previously connected session is lost; it must not fight explicit `DesiredVpnIntent::Disconnect`.

## Business Lane Model

Use a fixed lane set. Do not introduce a general-purpose unbounded thread pool.

| Lane | Worker | Actions |
| --- | --- | --- |
| `control` | Inline or one light worker | parse errors, `core.hello`, `window.setMode`, `window.resolveClosePrompt`; shell-only actions must complete without core RPC. |
| `read_model` | One worker | `status.get`, `vpn.status`, `runtime.status`, `service.status`, `helper.status`, `drivers.status`, `key.status`, `maintenance.inspectCore`; must read snapshots and return quickly. |
| `vpn_control` | One worker plus one owned session job | `vpn.connect`, `vpn.disconnect`, `vpn.authInteraction.get`, `vpn.authInteraction.respond`; the worker owns command serialization and cancellation, while network/session startup runs as a tracked job after Task 9. |
| `config_store` | One worker | `config.get`, `config.getAuth`, `config.saveAuth`, `config.getSettings`, `config.saveSettings`, `config.getKey`, `config.reset`, `config.import`, `config.export`, `routes.list`, `routes.add`, `routes.remove`, `routes.reset`, `key.reset`; serializes persistent state changes. |
| `diagnostics` | One worker | `logs.list` and future diagnostics reads; must not wait for `vpn_control`. |
| `platform_admin` | One worker | `service.install`, `service.uninstall`, `drivers.install`, backend/platform preflight checks, and stale-core destructive maintenance. |

Conflict rules:

- Same-lane requests run FIFO.
- Different-lane requests may run concurrently.
- `vpn.connect` while the VPN workflow is busy is coalesced into the latest desired intent and returns an accepted/coalesced response; it must not launch a second heavy workflow.
- `vpn.disconnect` while the VPN workflow is busy changes desired intent to disconnect, requests cancellation, and returns immediately as a normal user cancellation.
- `config.reset`, `key.reset`, and `maintenance.killStaleCore` are exclusive only for their lane; they must not block diagnostics reads.
- Response order is not guaranteed. Every response must carry its original `id` or `request_id`.

## File Structure Map

- Create `src/core/rpc/rpc_action_metadata.hpp` and `src/core/rpc/rpc_action_metadata.cpp`: action-to-lane metadata, conflict class, timeout class, and action classification helpers.
- Create `src/core/rpc/lane_scheduler.hpp` and `src/core/rpc/lane_scheduler.cpp`: fixed-lane worker scheduler, FIFO queues, shutdown behavior, and response callback execution.
- Modify `src/core/rpc/app_rpc_dispatcher.hpp` and `src/core/rpc/app_rpc_dispatcher.cpp`: register handlers with metadata, expose `metadata_for(action)`, keep `dispatch()` as a synchronous compatibility path for unit tests.
- Modify `src/core/rpc/desktop_rpc_adapter.hpp` and `src/core/rpc/desktop_rpc_adapter.cpp`: carry metadata for legacy desktop handlers.
- Modify `src/core/core_process.cpp`: parse on the front door, schedule business work by lane, write responses from callbacks via the existing `g_stdout_mutex`, remove all-business `g_desktop_dispatch_mutex`.
- Modify `src/app/ui_shell/core_rpc_client.hpp` and `src/app/ui_shell/core_rpc_client.cpp`: support multiple in-flight requests with a single reader pump and id-matched futures; keep `invoke()` as a blocking wrapper.
- Create `src/app/ui_shell/async_host_bridge.hpp` and `src/app/ui_shell/async_host_bridge.cpp`: accept host messages, handle local shell actions, invoke core asynchronously, and post response JSON through a platform callback.
- Modify `src/app/ui_shell/ui_shell_runtime.hpp` and `src/app/ui_shell/ui_shell_runtime.cpp`: wire `AsyncHostBridge` into `UiWindowConfig`.
- Modify platform hosts:
  - `src/platform/win32/ui_shell/webview2_host_win32.cpp`
  - `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
  - `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
  Each platform must post core responses later without blocking the WebView message callback.
- Modify `src/core/app_api/desktop_vpn_actions.cpp` and `src/core/tunnel_controller/*`: add test hooks first; later move connect to a tracked job contract.
- Create `src/core/tunnel_controller/connect_pipeline.hpp` and `src/core/tunnel_controller/connect_pipeline.cpp`: connect branch runner, first-failure coordinator, cancellation fan-out, branch timing.
- Create `src/core/tunnel_controller/connect_intent.hpp` and `src/core/tunnel_controller/connect_intent.cpp`: latest user intent, epoch reconciliation, busy-workflow coalescing, and pending in-memory connect request ownership.
- Create `src/core/tunnel_controller/native_handshake_job.hpp` and `src/core/tunnel_controller/native_handshake_job.cpp`: native auth/CSTP handshake without packet-device or route/DNS mutation.
- Modify `src/vpn_engine/native_engine.hpp`, `src/vpn_engine/native_engine.cpp`, `src/vpn_engine/protocol/session.hpp`, and `src/vpn_engine/protocol/session.cpp`: expose a two-phase native session boundary so auth/CSTP can finish before packet device attachment.
- Modify `src/platform/win32/driver_status.cpp`: replace or cache the double PowerShell/CIM adapter scan as a separate Windows readiness optimization after functional pipeline behavior is proven.
- Modify `webui/src/stores/vpn.ts`, `webui/src/pages/DashboardPage.vue`, and `webui/src/components/MinimalModeView.vue` only in Task 9 when `vpn.connect` changes from completed-result to accepted-job semantics.
- Modify `CMakeLists.txt`: add new source files and test targets.

## Task 1: Add Blocking Regression Tests

**Files:**
- Create: `tests/core_rpc_lane_scheduler_test.cpp`
- Modify: `tests/core_process_lifecycle_test.cpp`
- Modify: `tests/ui_shell_core_rpc_client_test.cpp`
- Modify: `tests/ui_shell_runtime_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Add scheduler RED tests**

Create `tests/core_rpc_lane_scheduler_test.cpp` with these concrete cases:

```cpp
// Expected final helper names from Task 3.
using exv::core_api::LaneScheduler;
using exv::core_api::RpcLane;
using exv::core_api::RpcRequest;
using exv::core_api::RpcResponse;

// 1. Same lane is FIFO.
// Schedule vpn_control request A that waits on a latch.
// Schedule vpn_control request B.
// Release A and assert completion order is A then B.

// 2. Different lanes run concurrently.
// Schedule vpn_control request A that blocks for 500 ms.
// Schedule diagnostics request B after A starts.
// Assert B completes before A is released.

// 3. VPN control requests remain serialized.
// Schedule vpn_control request A that waits on a latch.
// Schedule vpn_control request B that records "intent_update".
// Release A and assert B runs after A on the same lane.
// Latest-intent coalescing is tested at VpnConnectJobOwner level in Task 9,
// not inside the generic scheduler.

// 4. Shutdown drains accepted work and rejects new work.
// Stop scheduler.
// Assert schedule() returns false for a later diagnostics request.
```

Use `std::promise<void>` and `std::shared_future<void>` latches, not sleeps, for ordering.

- [ ] **Step 2: Add core process lane isolation RED test**

In `tests/core_process_lifecycle_test.cpp`, add a new block before the final SIGTERM-only test:

```cpp
// E2.3-lanes — long vpn.connect does not block logs.list.
// Arrange: install a test hook from Task 5 that blocks desktop vpn.connect
// after the request has entered the VPN lane.
// Act:
//   in_buf.feed(R"({"id":50,"action":"vpn.connect","payload":{"password":"x"}})" "\n");
//   wait until hook reports vpn.connect entered.
//   in_buf.feed(R"({"id":51,"action":"logs.list","payload":{}})" "\n");
// Assert:
//   logs.list id=51 is observed within 500 ms while vpn.connect is still blocked.
//   after releasing the hook, vpn.connect id=50 returns its own failure or status.
```

The first run must fail because `core_process_main()` currently dispatches and writes one request before reading the next request.

- [ ] **Step 3: Add core client out-of-order RED test**

Extend `tests/ui_shell_core_rpc_client_test.cpp` with a fake transport that receives two writes and emits reversed responses:

```cpp
// Request 101: logs.list
// Request 102: status.get
// Response order:
//   {"id":102,"ok":true,"data":{"connected":false}}
//   {"id":101,"ok":true,"data":[]}
// Assert invoke_async(101).get().id == 101 and invoke_async(102).get().id == 102.
```

The first run must fail because `CoreRpcClient::invoke()` returns the first non-event response to whichever caller is waiting.

- [ ] **Step 4: Add host callback non-blocking RED test**

Extend `tests/ui_shell_runtime_test.cpp` or create `tests/ui_shell_async_host_bridge_test.cpp`:

```cpp
// Fake transport blocks reads until a latch is released.
// Dispatch a renderer message for logs.list.
// Assert dispatch returns before the latch is released and before a core response exists.
// Release the latch.
// Assert FakeWindow received {"id":..., "ok":true, ...} via emit/post callback.
```

The first run must fail because current `FakeWindow::dispatch()` returns the synchronous response string.

- [ ] **Step 5: Wire tests and verify RED**

Add `core_rpc_lane_scheduler_test` and `ui_shell_async_host_bridge_test` to `CMakeLists.txt`. Run:

```powershell
cmake --build --preset windows-release --target core_rpc_lane_scheduler_test ui_shell_core_rpc_client_test ui_shell_runtime_test core_process_lifecycle_test
ctest --test-dir build-windows/cpp -R "core_rpc_lane_scheduler_test|ui_shell_core_rpc_client_test|ui_shell_runtime_test|core_process_lifecycle_test" --output-on-failure
```

Expected before implementation: new scheduler target does not build, out-of-order client test fails, host non-blocking test fails, and core process lane isolation test times out waiting for `logs.list`.

- [ ] **Step 6: Commit RED tests**

```powershell
git add tests/core_rpc_lane_scheduler_test.cpp tests/core_process_lifecycle_test.cpp tests/ui_shell_core_rpc_client_test.cpp tests/ui_shell_runtime_test.cpp CMakeLists.txt
git commit -m "test: capture core rpc lane blocking regressions"
```

## Task 2: Add Action Metadata

**Files:**
- Create: `src/core/rpc/rpc_action_metadata.hpp`
- Create: `src/core/rpc/rpc_action_metadata.cpp`
- Modify: `src/core/rpc/app_rpc_dispatcher.hpp`
- Modify: `src/core/rpc/app_rpc_dispatcher.cpp`
- Modify: `src/core/rpc/desktop_rpc_adapter.hpp`
- Modify: `src/core/rpc/desktop_rpc_adapter.cpp`
- Modify: `tests/app_api_rpc_dispatcher_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Extend dispatcher tests**

In `tests/app_api_rpc_dispatcher_test.cpp`, add assertions:

```cpp
AppRpcDispatcher dispatcher;
dispatcher.register_handler(
    "logs.list",
    [](const RpcRequest&) {
        RpcResponse resp;
        resp.success = true;
        resp.payload_json = "[]";
        return resp;
    },
    RpcActionMetadata{RpcLane::Diagnostics, RpcConflictClass::None});

auto meta = dispatcher.metadata_for("logs.list");
ok = expect(meta.has_value(), "logs.list metadata should be registered") && ok;
ok = expect(meta->lane == RpcLane::Diagnostics,
            "logs.list lane should be diagnostics") && ok;
ok = expect(!dispatcher.metadata_for("missing.action").has_value(),
            "missing action should have no metadata") && ok;
```

- [ ] **Step 2: Implement metadata types**

`src/core/rpc/rpc_action_metadata.hpp` must define:

```cpp
namespace exv::core_api {
enum class RpcLane { Control, ReadModel, VpnControl, ConfigStore, Diagnostics, PlatformAdmin };
enum class RpcConflictClass { None, VpnWorkflowIntent, ConfigWrite, PlatformAdminWrite };

struct RpcActionMetadata {
  RpcLane lane = RpcLane::ReadModel;
  RpcConflictClass conflict = RpcConflictClass::None;
  bool mutates_state = false;
};

RpcActionMetadata default_metadata_for_action(std::string_view action);
std::string_view lane_name(RpcLane lane);
}
```

The `default_metadata_for_action()` implementation must classify exactly:

```text
control: core.hello, window.setMode, window.resolveClosePrompt
read_model: status.get, vpn.status, runtime.status, service.status, helper.status, drivers.status, key.status, maintenance.inspectCore
vpn_control: vpn.connect, vpn.disconnect, vpn.authInteraction.get, vpn.authInteraction.respond
config_store: config.get, config.getAuth, config.saveAuth, config.getSettings, config.saveSettings, config.getKey, config.reset, config.import, config.export, routes.list, routes.add, routes.remove, routes.reset, key.reset
diagnostics: logs.list
platform_admin: service.install, service.uninstall, drivers.install, maintenance.killStaleCore
```

- [ ] **Step 3: Add metadata registration overload**

Update `AppRpcDispatcher`:

```cpp
void register_handler(const std::string& action,
                      Handler handler,
                      RpcActionMetadata metadata);
std::optional<RpcActionMetadata> metadata_for(std::string_view action) const;
```

Keep the existing two-argument `register_handler()` and have it call the new overload with `default_metadata_for_action(action)`.

- [ ] **Step 4: Preserve legacy adapter behavior**

Update `DesktopRpcAdapter::register_legacy_handler()` to accept an optional `RpcActionMetadata` with a default from `default_metadata_for_action(action)`. Existing callers continue to compile.

- [ ] **Step 5: Verify**

Run:

```powershell
cmake --build --preset windows-release --target app_api_rpc_dispatcher_test
ctest --test-dir build-windows/cpp -R app_api_rpc_dispatcher_test --output-on-failure
```

Expected: dispatcher tests pass and existing action registration remains source-compatible.

- [ ] **Step 6: Commit**

```powershell
git add src/core/rpc/rpc_action_metadata.hpp src/core/rpc/rpc_action_metadata.cpp src/core/rpc/app_rpc_dispatcher.hpp src/core/rpc/app_rpc_dispatcher.cpp src/core/rpc/desktop_rpc_adapter.hpp src/core/rpc/desktop_rpc_adapter.cpp tests/app_api_rpc_dispatcher_test.cpp CMakeLists.txt
git commit -m "core: classify rpc actions by business lane"
```

## Task 3: Implement Fixed Lane Scheduler

**Files:**
- Create: `src/core/rpc/lane_scheduler.hpp`
- Create: `src/core/rpc/lane_scheduler.cpp`
- Modify: `tests/core_rpc_lane_scheduler_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Implement scheduler API**

`lane_scheduler.hpp` must expose:

```cpp
namespace exv::core_api {
using RpcResponseCallback = std::function<void(RpcResponse)>;

struct LaneWorkItem {
  RpcRequest request;
  RpcActionMetadata metadata;
  std::function<RpcResponse(const RpcRequest&)> handler;
  RpcResponseCallback respond;
};

class LaneScheduler {
public:
  LaneScheduler();
  ~LaneScheduler();
  LaneScheduler(const LaneScheduler&) = delete;
  LaneScheduler& operator=(const LaneScheduler&) = delete;

  bool start();
  void stop();
  bool schedule(LaneWorkItem item);

private:
  struct LaneState;
  std::unique_ptr<LaneState> control_;
  std::unique_ptr<LaneState> read_model_;
  std::unique_ptr<LaneState> vpn_control_;
  std::unique_ptr<LaneState> config_store_;
  std::unique_ptr<LaneState> diagnostics_;
  std::unique_ptr<LaneState> platform_admin_;
};
}
```

- [ ] **Step 2: Implement worker behavior**

Each lane worker:

- waits on a condition variable,
- pops FIFO work,
- invokes `handler(request)`,
- copies `request.request_id` into the response when empty,
- calls `respond(response)` outside the queue lock,
- catches `std::exception` and returns `handler_exception`,
- exits cleanly on `stop()`.

The generic scheduler must not reject repeated `vpn.connect` requests. It only guarantees that `vpn_control` handlers run one at a time. Busy-workflow coalescing and latest-intent reconciliation belong to `VpnConnectJobOwner` / `VpnWorkflowIntent` in Task 9.

- [ ] **Step 3: Verify scheduler tests**

Run:

```powershell
cmake --build --preset windows-release --target core_rpc_lane_scheduler_test
ctest --test-dir build-windows/cpp -R core_rpc_lane_scheduler_test --output-on-failure
```

Expected: same-lane FIFO, different-lane concurrency, VPN-control serialization, and shutdown behavior pass.

- [ ] **Step 4: Commit**

```powershell
git add src/core/rpc/lane_scheduler.hpp src/core/rpc/lane_scheduler.cpp tests/core_rpc_lane_scheduler_test.cpp CMakeLists.txt
git commit -m "core: add fixed rpc lane scheduler"
```

## Task 4: Make Core Process Front Door Non-Blocking Across Lanes

**Files:**
- Modify: `src/core/core_process.cpp`
- Modify: `tests/core_process_lifecycle_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Refactor request parsing into scheduled work**

In `src/core/core_process.cpp`, keep JSON parsing on the stdin/pipe thread, then build `LaneWorkItem`:

```cpp
LaneWorkItem item;
item.request = make_rpc_request_from_wire(request);
item.metadata = dispatcher.metadata_for(item.request.action)
    .value_or(default_metadata_for_action(item.request.action));
item.handler = [&dispatcher, is_desktop](const RpcRequest& rpc_request) {
    return is_desktop ? dispatch_desktop_as_rpc(rpc_request)
                      : dispatcher.dispatch(rpc_request);
};
item.respond = [id, is_desktop](RpcResponse response) {
    write_json_line(is_desktop ? desktop_wire_response(id, response)
                               : native_wire_response(response));
};
```

Do not hold a process-wide desktop mutex around all desktop actions. Lane workers provide serialization where needed.

- [ ] **Step 2: Preserve synchronous helpers for tests**

Keep `handle_request_line()` as a testable helper by adding a synchronous fallback:

```cpp
static std::string handle_request_line_sync(AppRpcDispatcher& dispatcher,
                                            const std::string& request_line);
```

Use the scheduler path in `core_process_main()` and pipe listener dispatch. Unit tests that directly call the helper may use the sync helper.

- [ ] **Step 3: Ensure stdout writes are safe**

Keep `write_json_line()` and `g_stdout_mutex`. All scheduler callbacks must write through this function. No lane worker writes directly to `std::cout`.

- [ ] **Step 4: Verify core process tests**

Run:

```powershell
cmake --build --preset windows-release --target core_process_lifecycle_test
ctest --test-dir build-windows/cpp -R core_process_lifecycle_test --output-on-failure
```

Expected: existing lifecycle tests pass and the new E2.3 lane test now observes `logs.list` while `vpn.connect` is blocked.

- [ ] **Step 5: Commit**

```powershell
git add src/core/core_process.cpp tests/core_process_lifecycle_test.cpp CMakeLists.txt
git commit -m "core: route process requests through lane scheduler"
```

## Task 5: Add Deterministic VPN Blocking Test Hook

**Files:**
- Modify: `src/core/app_api/desktop_vpn_actions.cpp`
- Create: `src/core/app_api/desktop_vpn_test_hooks.hpp`
- Modify: `tests/core_process_lifecycle_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Add test-only hook declaration**

Create `src/core/app_api/desktop_vpn_test_hooks.hpp`:

```cpp
#pragma once

#include <functional>

namespace ecnuvpn::app_api::testing {
using DesktopVpnConnectEnteredHook = std::function<void()>;
void set_desktop_vpn_connect_entered_hook(DesktopVpnConnectEnteredHook hook);
void fire_desktop_vpn_connect_entered_hook();
}
```

- [ ] **Step 2: Implement hook in desktop VPN actions**

In `desktop_vpn_actions.cpp`, add the storage and fire the hook immediately after logging `app_api: vpn.connect entry` and before expensive preflight:

```cpp
namespace ecnuvpn::app_api::testing {
namespace {
std::mutex g_hook_mutex;
DesktopVpnConnectEnteredHook g_connect_entered_hook;
}

void set_desktop_vpn_connect_entered_hook(DesktopVpnConnectEnteredHook hook) {
  std::lock_guard<std::mutex> lock(g_hook_mutex);
  g_connect_entered_hook = std::move(hook);
}

void fire_desktop_vpn_connect_entered_hook() {
  DesktopVpnConnectEnteredHook hook;
  {
    std::lock_guard<std::mutex> lock(g_hook_mutex);
    hook = g_connect_entered_hook;
  }
  if (hook) hook();
}
}
```

The hook is harmless in production because it is empty unless a test installs it.

- [ ] **Step 3: Use the hook in E2.3**

In `tests/core_process_lifecycle_test.cpp`, install a hook that signals `entered` and waits for `release`:

```cpp
std::promise<void> entered;
std::promise<void> release;
auto release_future = release.get_future().share();
ecnuvpn::app_api::testing::set_desktop_vpn_connect_entered_hook([&] {
    entered.set_value();
    release_future.wait();
});
```

After the assertions, clear the hook:

```cpp
ecnuvpn::app_api::testing::set_desktop_vpn_connect_entered_hook(nullptr);
```

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build --preset windows-release --target core_process_lifecycle_test
ctest --test-dir build-windows/cpp -R core_process_lifecycle_test --output-on-failure
```

Expected: the test deterministically blocks `vpn.connect` and proves `logs.list` responds before release.

- [ ] **Step 5: Commit**

```powershell
git add src/core/app_api/desktop_vpn_actions.cpp src/core/app_api/desktop_vpn_test_hooks.hpp tests/core_process_lifecycle_test.cpp CMakeLists.txt
git commit -m "test: add deterministic vpn connect blocking hook"
```

## Task 6: Make CoreRpcClient Support Concurrent Requests

**Files:**
- Modify: `src/app/ui_shell/core_rpc_client.hpp`
- Modify: `src/app/ui_shell/core_rpc_client.cpp`
- Modify: `tests/ui_shell_core_rpc_client_test.cpp`

- [ ] **Step 1: Add async API**

Extend `CoreRpcClient`:

```cpp
std::future<CoreRpcResponse> invoke_async(CoreRpcRequest request);
void shutdown();
```

Internals:

- `write_mutex_` protects `transport_.write_line()`.
- `pending_mutex_` protects `std::map<std::string, std::promise<CoreRpcResponse>> pending_`.
- `reader_thread_` is started on first `invoke_async()`.
- reader loop calls `transport_.read_line()`, parses events, parses responses, resolves matching promise by `request_id` or `id`.
- `invoke()` calls `invoke_async(request).get()` to keep old tests and call sites source-compatible.

- [ ] **Step 2: Add closed-transport failure behavior**

If `write_line()` fails, return a ready future with:

```cpp
ok=false
code="transport_closed"
message="Core RPC transport is closed"
```

If the reader detects transport close, resolve every pending promise with the same error code.

- [ ] **Step 3: Verify client tests**

Run:

```powershell
cmake --build --preset windows-release --target ui_shell_core_rpc_client_test
ctest --test-dir build-windows/cpp -R ui_shell_core_rpc_client_test --output-on-failure
```

Expected: existing serialization/error tests pass and the new out-of-order test passes.

- [ ] **Step 4: Commit**

```powershell
git add src/app/ui_shell/core_rpc_client.hpp src/app/ui_shell/core_rpc_client.cpp tests/ui_shell_core_rpc_client_test.cpp
git commit -m "ui-shell: route core rpc responses by request id"
```

## Task 7: Add Async Host Bridge

**Files:**
- Create: `src/app/ui_shell/async_host_bridge.hpp`
- Create: `src/app/ui_shell/async_host_bridge.cpp`
- Modify: `src/app/ui_shell/host_bridge.hpp`
- Modify: `src/app/ui_shell/host_bridge.cpp`
- Modify: `src/app/ui_shell/ui_shell_runtime.hpp`
- Modify: `src/app/ui_shell/ui_shell_runtime.cpp`
- Modify: `tests/ui_shell_runtime_test.cpp`
- Create: `tests/ui_shell_async_host_bridge_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Define bridge API**

`async_host_bridge.hpp`:

```cpp
namespace ecnuvpn::ui_shell {
using HostResponsePoster = std::function<void(std::string)>;

class AsyncHostBridge {
public:
  AsyncHostBridge(CoreRpcClient& client, HostResponsePoster post_response);
  ~AsyncHostBridge();

  bool accept_message(std::string message_json);
  void shutdown();

private:
  CoreRpcClient& client_;
  HostResponsePoster post_response_;
  std::atomic<bool> stopped_{false};
};
}
```

`accept_message()` returns `true` after it accepts or locally rejects a message. It must never wait for a core response.

- [ ] **Step 2: Keep local shell actions local**

Move the existing local handling for these actions into shared host code so all platforms behave the same:

```text
window.setMode
window.resolveClosePrompt
```

The bridge posts an immediate response for local actions and does not call `CoreRpcClient`.

- [ ] **Step 3: Post async core responses**

For core actions:

```cpp
auto future = client_.invoke_async(request);
std::thread([future = std::move(future),
             id,
             poster = post_response_,
             stopped = &stopped_]() mutable {
  CoreRpcResponse response = future.get();
  if (stopped->load()) return;
  poster(host_wire_response(id, response));
}).detach();
```

Use a small owned joinable worker list instead of detached threads if the implementation needs deterministic teardown; the public behavior must remain the same: the WebView callback is not blocked.

- [ ] **Step 4: Wire runtime**

Update `run_ui_shell_window()` so the window message handler calls `bridge.accept_message(message_json)` and returns an immediate host-accepted response only for platforms/tests that still require a returned string:

```json
{"id":0,"ok":true,"data":{"accepted":true}}
```

Platform hosts that already post responses should call `accept_message()` and rely on `post_response_`.

- [ ] **Step 5: Verify host tests**

Run:

```powershell
cmake --build --preset windows-release --target ui_shell_runtime_test ui_shell_async_host_bridge_test
ctest --test-dir build-windows/cpp -R "ui_shell_runtime_test|ui_shell_async_host_bridge_test" --output-on-failure
```

Expected: a slow fake core no longer blocks message acceptance, local actions return without transport writes, and async responses are posted by id.

- [ ] **Step 6: Commit**

```powershell
git add src/app/ui_shell/async_host_bridge.hpp src/app/ui_shell/async_host_bridge.cpp src/app/ui_shell/host_bridge.hpp src/app/ui_shell/host_bridge.cpp src/app/ui_shell/ui_shell_runtime.hpp src/app/ui_shell/ui_shell_runtime.cpp tests/ui_shell_runtime_test.cpp tests/ui_shell_async_host_bridge_test.cpp CMakeLists.txt
git commit -m "ui-shell: add async host rpc bridge"
```

## Task 8: Wire Platform Hosts To Async Posting

**Files:**
- Modify: `src/platform/win32/ui_shell/webview2_host_win32.cpp`
- Modify: `src/platform/darwin/ui_shell/wk_webview_host_darwin.mm`
- Modify: `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- Modify: platform runtime tests near:
  - `tests/win32_webview2_runtime_test.cpp`
  - `tests/darwin_wkwebview_runtime_test.cpp`
  - `tests/linux_webkitgtk_runtime_test.cpp`

- [ ] **Step 1: Update Windows host**

In `WebView2Window::handle_web_message()`, keep local `window.setMode` and close prompt handling. For all other actions, call async bridge acceptance and return `S_OK` immediately. Responses are posted by:

```cpp
webview_->PostWebMessageAsJson(wide_response.c_str());
```

from the bridge poster callback.

- [ ] **Step 2: Update Darwin host**

In `WkWebViewWindow::handle_script_message()`, call async bridge acceptance and return immediately. The poster callback must dispatch back to the main queue before `post_json_to_renderer(...)`:

```objc
dispatch_async(dispatch_get_main_queue(), ^{
  post_json_to_renderer(response_json);
});
```

- [ ] **Step 3: Update Linux host**

In `WebKitGtkWindow::handle_script_message()`, call async bridge acceptance and return immediately. The poster callback must use `g_main_context_invoke(nullptr, ...)` or an existing GTK-safe posting path before `post_json_to_renderer(...)`.

- [ ] **Step 4: Verify platform tests**

Run available platform tests for the current OS plus cross-platform stub tests:

```powershell
cmake --build --preset windows-release --target ui_shell_runtime_test win32_webview2_runtime_test
ctest --test-dir build-windows/cpp -R "ui_shell_runtime_test|win32_webview2_runtime_test" --output-on-failure
```

On macOS/Linux agents, replace `win32_webview2_runtime_test` with the matching platform test target.

- [ ] **Step 5: Commit**

```powershell
git add src/platform/win32/ui_shell/webview2_host_win32.cpp src/platform/darwin/ui_shell/wk_webview_host_darwin.mm src/platform/linux/ui_shell/webkitgtk_host_linux.cpp tests/win32_webview2_runtime_test.cpp tests/darwin_wkwebview_runtime_test.cpp tests/linux_webkitgtk_runtime_test.cpp
git commit -m "ui-shell: post platform host responses asynchronously"
```

## Task 9: Convert VPN Connect To Accepted Parallel Pipeline

**Files:**
- Create: `src/core/tunnel_controller/vpn_connect_job.hpp`
- Create: `src/core/tunnel_controller/vpn_connect_job.cpp`
- Create: `src/core/tunnel_controller/connect_intent.hpp`
- Create: `src/core/tunnel_controller/connect_intent.cpp`
- Create: `src/core/tunnel_controller/connect_pipeline.hpp`
- Create: `src/core/tunnel_controller/connect_pipeline.cpp`
- Create: `src/core/tunnel_controller/native_handshake_job.hpp`
- Create: `src/core/tunnel_controller/native_handshake_job.cpp`
- Modify: `src/core/app_api/desktop_vpn_actions.cpp`
- Modify: `src/core/rpc/vpn_actions.cpp`
- Modify: `src/core/tunnel_controller/core_session_runner.hpp`
- Modify: `src/core/tunnel_controller/core_session_runner.cpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_connect.cpp`
- Modify: `src/core/tunnel_controller/tunnel_controller_disconnect.cpp`
- Modify: `src/vpn_engine/native_engine.hpp`
- Modify: `src/vpn_engine/native_engine.cpp`
- Modify: `src/vpn_engine/protocol/session.hpp`
- Modify: `src/vpn_engine/protocol/session.cpp`
- Modify: `tests/connection_attempt_test.cpp`
- Modify: `tests/core_api/vpn_actions_test.cpp`
- Create: `tests/connect_intent_test.cpp`
- Create: `tests/connect_pipeline_test.cpp`
- Modify: `tests/core_session_runner_test.cpp`
- Modify: `tests/core_process_lifecycle_test.cpp`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/pages/DashboardPage.vue`
- Modify: `webui/src/components/MinimalModeView.vue`

- [ ] **Step 1: Add accepted response and first-failure tests**

Extend `tests/core_api/vpn_actions_test.cpp`:

```cpp
auto resp = fix.dispatch("vpn.connect", R"({"profile_id":"test","auto_reconnect":false})");
ok = expect(resp.success, "vpn.connect should accept a valid connection job") && ok;
auto payload = json::parse(resp.payload_json);
ok = expect(payload.value("accepted", false), "vpn.connect returns accepted=true") && ok;
ok = expect(payload.value("phase", "") == "connecting",
            "vpn.connect returns connecting phase") && ok;
ok = expect(payload.contains("job_id"), "vpn.connect returns job_id") && ok;
```

Add busy-workflow coalescing assertion:

```cpp
auto second = fix.dispatch("vpn.connect", R"({"profile_id":"test","auto_reconnect":false})");
ok = expect(second.success, "busy vpn.connect should be accepted as intent update") && ok;
auto second_payload = json::parse(second.payload_json);
ok = expect(second_payload.value("accepted", false),
            "busy vpn.connect returns accepted=true") && ok;
ok = expect(second_payload.value("coalesced", false),
            "busy vpn.connect returns coalesced=true") && ok;
ok = expect(second_payload.value("desired_connected", false),
            "busy vpn.connect keeps desired_connected=true") && ok;
ok = expect(second_payload.value("active_job_id", "") == payload.value("job_id", ""),
            "busy vpn.connect references active job instead of starting a duplicate") && ok;
```

Create `tests/connect_pipeline_test.cpp` with deterministic branch latches:

```cpp
// Branch A: backend_helper_ready waits on helper_release.
// Branch B: platform_ready returns failure "wintun_missing" immediately.
// Branch C: protocol_handshake waits on handshake_release.
//
// Assert:
// - coordinator result is failure code "wintun_missing" within 200 ms.
// - cancellation was requested for backend and handshake branches.
// - releasing backend/handshake later does not change the user-visible result.
// - a later non-cancel failure records one diagnostic late-failure event.
```

Add a success rendezvous case:

```cpp
// All three branches succeed in different orders.
// Assert the pipeline calls the serial attach/network tail exactly once after
// all branch results are present, not before.
```

Create `tests/connect_intent_test.cpp`:

```cpp
// connect_when_idle_starts_job:
//   submit connect intent epoch 1.
//   assert desired=Connect, active job created, response accepted=true.
//
// cancel_while_connecting_is_normal:
//   submit connect intent epoch 1 and hold the job in protocol_handshake.
//   submit disconnect intent epoch 2.
//   assert cancellation requested, response accepted=true, cancelling=true,
//   user_cancelled=true, no last_error is set, and no failure log is emitted.
//
// rapid_clicks_coalesce_to_latest_disconnect:
//   submit connect, disconnect, connect, disconnect while the first job is busy.
//   assert only one active job exists and latest desired intent is Disconnect.
//   release cleanup; assert no new connect job starts.
//
// rapid_clicks_coalesce_to_latest_connect_after_cleanup:
//   submit connect, disconnect, connect while the first job is cancelling.
//   assert only one active job exists before cleanup completes.
//   release cleanup; assert exactly one new connect job starts with the newer epoch.
//
// failed_job_does_not_loop_without_new_epoch:
//   submit connect epoch 1 and make the job fail with auth_failed.
//   assert no automatic retry starts because epoch 1 was already represented.
//   submit connect epoch 2; assert a new job starts.
```

- [ ] **Step 2: Implement job owner and branch coordinator**

`vpn_connect_job.hpp`:

```cpp
enum class DesiredVpnIntent { Disconnect, Connect };

struct PendingConnectRequest {
  std::string profile_id;
  std::string server;
  bool has_password = false;
};

struct VpnWorkflowIntent {
  DesiredVpnIntent desired = DesiredVpnIntent::Disconnect;
  std::uint64_t epoch = 0;
  PendingConnectRequest pending_connect;
};

struct VpnConnectJobState {
  std::string job_id;
  std::string phase;
  bool active = false;
  bool cancelling = false;
  bool user_cancelled = false;
  bool coalesced = false;
  bool desired_connected = false;
  std::uint64_t intent_epoch = 0;
  std::string last_error_code;
  std::string last_error_message;
};

class VpnConnectJobOwner {
public:
  VpnConnectJobState submit_connect(PendingConnectRequest request,
                                    std::function<void(std::stop_token,
                                                       std::uint64_t)> run);
  VpnConnectJobState submit_disconnect(std::string reason);
  VpnConnectJobState snapshot() const;
  bool request_cancel(std::string reason);
  void reconcile_after_idle();
};
```

Use `std::jthread` where available under C++20. The job owner has one active job maximum. The latest intent and epoch live in `connect_intent.hpp/.cpp`; the owner must not log or expose VPN passwords.

`connect_pipeline.hpp`:

```cpp
namespace exv::core {
enum class ConnectBranch { BackendHelperReady, PlatformReady, ProtocolHandshake };

struct ConnectBranchResult {
  ConnectBranch branch;
  bool ok = false;
  std::string code;
  std::string message;
  nlohmann::json payload = nlohmann::json::object();
};

struct ConnectPipelineResult {
  bool ok = false;
  std::string first_failure_branch;
  std::string code;
  std::string message;
  nlohmann::json backend;
  nlohmann::json platform;
  nlohmann::json handshake;
};

class ConnectPipeline {
public:
  using BranchFn = std::function<ConnectBranchResult(std::stop_token)>;
  using LateFailureLogger = std::function<void(const ConnectBranchResult&,
                                               std::string_view first_code)>;

  ConnectPipeline(std::string job_id, LateFailureLogger logger);
  ConnectPipelineResult run(BranchFn backend_helper,
                            BranchFn platform_ready,
                            BranchFn protocol_handshake,
                            std::stop_token external_stop);
};
}
```

The coordinator returns as soon as the first branch fails and requests stop for the other branches. It waits only for branch threads that have already entered cleanup code required by RAII-owned resources; it must not wait for long network/auth operations after a user-visible failure is decided.

- [ ] **Step 3: Split native handshake from packet/device attach**

Add a two-phase native boundary:

```cpp
struct NativeHandshakeResult {
  bool ok = false;
  std::string code;
  std::string message;
  ecnuvpn::vpn_engine::TunnelMetadata metadata;
};

class NativeHandshakeJob {
public:
  NativeHandshakeResult run(const ecnuvpn::Config& cfg,
                            const std::string& password,
                            std::stop_token stop);
  bool attach_packet_loop(std::function<ecnuvpn::vpn_engine::ValidationResult(
                          const ecnuvpn::vpn_engine::TunnelMetadata&,
                          ecnuvpn::vpn_engine::DeviceConfig*)> network_configurator);
  void stop();
};
```

Refactor `NativeVpnEngineSession::start()` so auth and CSTP can be performed without opening a packet device:

```cpp
ValidationResult NativeVpnEngineSession::start_handshake(TunnelMetadata* metadata);
ValidationResult NativeVpnEngineSession::start_packet_loop(DeviceConfig config);
```

Keep existing `start()` as a compatibility wrapper:

```cpp
ValidationResult NativeVpnEngineSession::start() {
  TunnelMetadata metadata;
  auto handshake = start_handshake(&metadata);
  if (!handshake.ok) return handshake;
  DeviceConfig config = device_config_from_metadata(metadata);
  if (dependencies_.network_configurator) {
    auto network = dependencies_.network_configurator(metadata, &config);
    if (!network.ok) return network;
  }
  return start_packet_loop(config);
}
```

The handshake phase must not call `dependencies_.network_configurator`, `packet_device_factory`, `PacketDevice::open`, helper `PrepareTunnelDevice`, helper `ApplyTunnelConfig`, route changes, or DNS changes.

- [ ] **Step 4: Run connect branches concurrently**

After quick config/password validation and connection-attempt lock acquisition, `vpn.connect` starts the connect job and returns accepted immediately:

```json
{"accepted":true,"phase":"connecting","job_id":"..."}
```

Inside the job, run these three branches:

```cpp
auto backend_helper = [&](std::stop_token stop) {
  // resolve_backend(options)
  // ensure helper pipe connection
  // helper Hello
  // acquire core lease
  // helper StartSession
};

auto platform_ready = [&](std::stop_token stop) {
  // runtime_status_json(cfg)
  // preflight_connect_platform_checks(to_platform_config_view(cfg))
};

auto protocol_handshake = [&](std::stop_token stop) {
  // NativeHandshakeJob::run(cfg, password, stop)
};
```

If all succeed, the job enters the serial tail:

```cpp
// ensure/reuse TunnelController with prepared helper client/session
// attach prepared NativeHandshakeJob to CoreSessionRunner
// prepare_tunnel_device
// apply_tunnel_config
// start packet loop
```

This tail is intentionally not parallel: route/DNS/adapter mutation must happen only after auth/CSTP metadata, helper session, and platform readiness are all valid.

- [ ] **Step 5: Keep disconnect and cancel responsive**

`vpn.disconnect` calls `VpnConnectJobOwner::submit_disconnect("user_cancelled_connect")` before controller disconnect when the active phase is pre-connected. It must return while the connect job is still unwinding:

```json
{"accepted":true,"cancelling":true,"user_cancelled":true,"desired_connected":false}
```

If the VPN is already connected, `vpn.disconnect` keeps the existing disconnect semantics and tears down the active tunnel. If the workflow is cancelling or disconnecting and the user clicks connect again, `vpn.connect` updates the latest intent to connect and returns:

```json
{"accepted":true,"coalesced":true,"desired_connected":true,"active_job_id":"..."}
```

The reconnect is started only after current cleanup reaches idle.

- [ ] **Step 6: Update frontend store**

In `webui/src/stores/vpn.ts`, treat connect success with `accepted=true` as an in-flight connection, then rely on status polling/events to clear `connectInFlight`. Treat failed accepted-job completion through status/error event paths, not by waiting for the original `connect` promise to carry the final failure.

Add a `cancelConnect()` action used by the yellow in-progress button. It sends `vpn.disconnect` while `connectInFlight` is true, immediately clears password prompt state, suppresses error modal behavior for `user_cancelled=true`, and moves the UI into the disconnected/cancelled visual state. Repeated click behavior:

```ts
// connect -> cancel -> connect -> cancel
// UI follows the latest click immediately.
// Backend receives intent updates; it does not start parallel duplicate workflows.
```

- [ ] **Step 7: Verify**

Run:

```powershell
cmake --build --preset windows-release --target connect_intent_test connect_pipeline_test vpn_actions_test connection_attempt_test core_session_runner_test core_process_lifecycle_test
ctest --test-dir build-windows/cpp -R "connect_intent_test|connect_pipeline_test|vpn_actions_test|connection_attempt_test|core_session_runner_test|core_process_lifecycle_test" --output-on-failure
pnpm --dir webui test:host
pnpm --dir webui exec vue-tsc -b
```

Expected: connect RPC returns promptly, busy connect/cancel clicks coalesce into latest intent, user cancellation is not reported as an error, helper/platform/protocol branches run concurrently, first failure reaches the UI before slower branches finish, late branch failures are logged once, disconnect remains responsive, and frontend loading/error state stays correct.

- [ ] **Step 8: Commit**

```powershell
git add src/core/tunnel_controller/vpn_connect_job.hpp src/core/tunnel_controller/vpn_connect_job.cpp src/core/tunnel_controller/connect_intent.hpp src/core/tunnel_controller/connect_intent.cpp src/core/tunnel_controller/connect_pipeline.hpp src/core/tunnel_controller/connect_pipeline.cpp src/core/tunnel_controller/native_handshake_job.hpp src/core/tunnel_controller/native_handshake_job.cpp src/core/app_api/desktop_vpn_actions.cpp src/core/rpc/vpn_actions.cpp src/core/tunnel_controller src/vpn_engine tests webui/src/stores/vpn.ts webui/src/pages/DashboardPage.vue webui/src/components/MinimalModeView.vue CMakeLists.txt
git commit -m "core: run vpn connect through parallel pipeline"
```

## Task 10: Optimize Windows Platform Readiness Probe

**Files:**
- Modify: `src/platform/win32/driver_status.cpp`
- Modify: `src/platform/win32/app_api_runtime_policy.cpp`
- Modify: `src/platform/common/runtime_status.cpp`
- Modify: `tests/app_api_runtime_policy_test.cpp`
- Modify: `tests/win32_platform_network_ops_test.cpp`
- Create: `tests/win32_driver_status_test.cpp`
- Modify: `CMakeLists.txt`

- [ ] **Step 1: Add Windows driver probe timing tests**

Create `tests/win32_driver_status_test.cpp` with dependency injection around adapter enumeration:

```cpp
// Fake enumerator records calls.
// Calling driver_status_json(cfg) must enumerate adapters once total, not once
// for Wintun and once for TAP.
// Calling preflight_connect_platform_checks(cfg) with windows_tunnel_driver
// set to "wintun" and bundled wintun present must not enumerate TAP adapters.
// Calling it with driver "tap" must enumerate TAP and must not require Wintun.
```

- [ ] **Step 2: Replace double PowerShell scan**

Refactor `list_windows_adapters("wintun")` and `list_windows_adapters("tap")` into one native or single-command snapshot:

```cpp
struct WindowsAdapterSnapshot {
  std::vector<std::string> wintun_adapters;
  std::vector<std::string> tap_adapters;
};

WindowsAdapterSnapshot list_windows_tunnel_adapters();
```

If native `GetAdaptersAddresses` or SetupAPI coverage is implemented, prefer it over PowerShell. If PowerShell remains as a temporary fallback, run one `Get-CimInstance Win32_NetworkAdapter` command and classify both Wintun and TAP from the same output.

- [ ] **Step 3: Add short-lived cache**

Cache driver readiness for a narrow TTL inside the core process:

```cpp
struct DriverStatusCacheEntry {
  nlohmann::json status;
  std::chrono::steady_clock::time_point created_at;
};
```

Use a TTL of 2 seconds for connect preflight/status bursts. Invalidate the cache after `drivers.install`, service install/uninstall, and any explicit settings save that changes `windows_tunnel_driver` or `windows_tap_interface`.

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build --preset windows-release --target win32_driver_status_test app_api_runtime_policy_test
ctest --test-dir build-windows/cpp -R "win32_driver_status_test|app_api_runtime_policy_test" --output-on-failure
```

Expected: Windows preflight no longer runs two adapter enumeration processes per connect, and driver-status payload semantics remain unchanged.

- [ ] **Step 5: Commit**

```powershell
git add src/platform/win32/driver_status.cpp src/platform/win32/app_api_runtime_policy.cpp src/platform/common/runtime_status.cpp tests/win32_driver_status_test.cpp tests/app_api_runtime_policy_test.cpp tests/win32_platform_network_ops_test.cpp CMakeLists.txt
git commit -m "platform: speed up windows driver readiness checks"
```

## Task 11: Remove Redundant UI Mode/Core Refresh Coupling

**Files:**
- Modify: `webui/src/stores/ui.ts`
- Modify: `webui/src/App.vue`
- Modify: `webui/host/__tests__/ui-mode-and-connect-failure.test.ts`
- Modify: `src/platform/linux/ui_shell/webkitgtk_host_linux.cpp`
- Modify: `tests/linux_webkitgtk_runtime_test.cpp`

- [ ] **Step 1: Extend frontend mode tests**

Add a host test that toggles mode four times while a core RPC promise is pending:

```ts
await ui.setMode('minimal')
await ui.setMode('advanced')
await ui.setMode('minimal')
await ui.setMode('advanced')
resolvePendingStatusGetWith({ minimal_mode: true })
assert.equal(ui.mode, 'advanced')
```

This locks the rule that stale backend/UI-shell refreshes cannot overwrite the newest frontend mode.

- [ ] **Step 2: Ensure Linux local mode handling**

Mirror Windows/macOS local handling for `window.setMode` in Linux host code so no platform sends mode changes through core.

- [ ] **Step 3: Verify**

Run:

```powershell
pnpm --dir webui exec node scripts/run-host-test.cjs host/__tests__/ui-mode-and-connect-failure.test.ts
cmake --build --preset windows-release --target linux_webkitgtk_runtime_test
ctest --test-dir build-windows/cpp -R linux_webkitgtk_runtime_test --output-on-failure
```

Expected: frontend mode remains last-writer-wins locally even when delayed core/status results arrive.

- [ ] **Step 4: Commit**

```powershell
git add webui/src/stores/ui.ts webui/src/App.vue webui/host/__tests__/ui-mode-and-connect-failure.test.ts src/platform/linux/ui_shell/webkitgtk_host_linux.cpp tests/linux_webkitgtk_runtime_test.cpp
git commit -m "ui: keep shell mode isolated from core refreshes"
```

## Task 12: Add Timing And Contract Guardrails

**Files:**
- Modify: `tests/core_architecture_contract_test.cpp`
- Modify: `tests/app_api_status_contract_test.cpp`
- Modify: `tests/security/no_secret_in_logs_test.cpp`
- Modify: `src/core/tunnel_controller/timing.cpp`
- Modify: `src/core/app_api/desktop_vpn_actions.cpp`
- Modify: `src/core/tunnel_controller/connect_pipeline.cpp`

- [ ] **Step 1: Guard against global desktop serialization**

In `tests/core_architecture_contract_test.cpp`, fail if `src/core/core_process.cpp` contains:

```text
g_desktop_dispatch_mutex
std::lock_guard<std::mutex> lock(g_desktop_dispatch_mutex)
```

and require references to:

```text
LaneScheduler
default_metadata_for_action
write_json_line
ConnectPipeline
```

- [ ] **Step 2: Add timing duplicate guard**

Add a test that scans the connect timing formatter and asserts a stage is emitted once per event sink. The log sample that motivated this work had duplicate lines; duplicate emission should be intentional only when there are two sinks, not two identical core events.

- [ ] **Step 3: Add branch timing guard**

Add a test that requires connect timing output to include independent branch names:

```text
connect_pipeline.backend_helper_ready
connect_pipeline.platform_ready
connect_pipeline.protocol_handshake
connect_pipeline.first_failure
connect_pipeline.serial_tail
```

The pipeline must log branch start/finish and first-failure cancellation without logging successful discarded branch results.

- [ ] **Step 4: Verify**

Run:

```powershell
cmake --build --preset windows-release --target core_architecture_contract_test app_api_status_contract_test no_secret_in_logs_test connect_pipeline_test
ctest --test-dir build-windows/cpp -R "core_architecture_contract_test|app_api_status_contract_test|no_secret_in_logs_test|connect_pipeline_test" --output-on-failure
```

Expected: architecture guardrails reject reintroducing one global desktop dispatch lock, timing logs do not duplicate a single event, and connect branch timings are separately visible.

- [ ] **Step 5: Commit**

```powershell
git add tests/core_architecture_contract_test.cpp tests/app_api_status_contract_test.cpp tests/security/no_secret_in_logs_test.cpp src/core/tunnel_controller/timing.cpp src/core/tunnel_controller/connect_pipeline.cpp src/core/app_api/desktop_vpn_actions.cpp
git commit -m "test: guard rpc lane and connect pipeline contracts"
```

## Task 13: Final Verification And Manual Debug Pass

**Files:**
- Modify docs only if verification reveals stale behavior.

- [ ] **Step 1: Run focused C++ tests**

```powershell
cmake --build --preset windows-release --target core_rpc_lane_scheduler_test connect_intent_test connect_pipeline_test core_process_lifecycle_test ui_shell_core_rpc_client_test ui_shell_runtime_test app_api_rpc_dispatcher_test core_architecture_contract_test vpn_actions_test connection_attempt_test win32_driver_status_test
ctest --test-dir build-windows/cpp -R "core_rpc_lane_scheduler_test|connect_intent_test|connect_pipeline_test|core_process_lifecycle_test|ui_shell_core_rpc_client_test|ui_shell_runtime_test|app_api_rpc_dispatcher_test|core_architecture_contract_test|vpn_actions_test|connection_attempt_test|win32_driver_status_test" --output-on-failure
```

- [ ] **Step 2: Run frontend checks**

```powershell
pnpm --dir webui test:host
pnpm --dir webui exec vue-tsc -b
pnpm --dir webui run build
```

- [ ] **Step 3: Run release-blocking suite**

```powershell
./scripts/run-tests.ps1 -Preset windows-release -Label release-blocking
```

- [ ] **Step 4: Manual debug reproduction**

Start the desktop shell, click connect once, click the yellow in-progress button to cancel, rapidly click connect/cancel a few times, open logs while connection is still in progress, then toggle minimal/advanced four times. Expected manual results:

- logs panel opens and refreshes while connect is still running,
- UI mode remains on the fourth click result with no delayed bounce,
- a failed native session surfaces an error modal or visible error state,
- user cancellation during connect immediately changes the UI to the disconnected/cancelled visual state and does not show an error modal,
- rapid connect/cancel clicks coalesce to the latest user intent without starting duplicate workflows,
- first-failure branch errors return to UI without waiting for slower branch cleanup,
- `connect-timing` lines show the long VPN phase without blocking `logs.list`,
- `connect-timing` includes separate backend/helper, platform, protocol-handshake, and serial-tail branch timings,
- `window.setMode` does not appear in core RPC traces.

- [ ] **Step 5: Record verification**

Create a report under `docs/superpowers/reports/` with commit hash, focused command output summaries, release-blocking result, and manual debug observations.

- [ ] **Step 6: Commit verification docs**

```powershell
git add docs/superpowers/reports/<created-report>.md
git commit -m "docs: record core rpc lane isolation verification"
```

## Self-Review

- Spec coverage: The plan covers frontend non-blocking behavior, core region isolation, fixed business-area workers, VPN connect accepted-job semantics, latest user intent coalescing, user cancellation without error reporting, parallel backend/platform/protocol readiness, first-failure cancellation, Windows driver-check latency, log/status/config availability during connect, UI mode locality, service prompt/frontend-local state compatibility, and timing/debug guardrails.
- Placeholder scan: No task uses banned placeholder markers or unspecified test commands. Every code-changing task names exact files, behavior, commands, and expected outcomes.
- Type consistency: `RpcLane`, `RpcConflictClass`, `RpcActionMetadata`, `LaneScheduler`, `LaneWorkItem`, `AsyncHostBridge`, `DesiredVpnIntent`, `VpnWorkflowIntent`, `VpnConnectJobOwner`, `ConnectPipeline`, `ConnectBranchResult`, and `NativeHandshakeJob` are defined before later tasks refer to them.
