# Helper 生命周期管理与心跳机制修复计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [`) syntax for tracking.

**Goal:** 修复 helper 进程泄漏问题——helper 应仅负责特权操作（网卡创建、路由写入），具备有限重试能力，并通过心跳机制定期向 core 确认自身是否应存续，core 无响应时自销毁退出。

**Architecture:**
- helper 是一个 IPC 服务端，监听 core 的请求。当前的 oneshot helper 在 worker 失败后仍可能残留（supervisor 进程无限重连）。
- 新增心跳机制：oneshot helper 定期向 core 发送心跳，core 返回确认则继续存续，超时无响应则自销毁。
- 重试上限：supervisor 重连默认上限 3 次（可通过 retry_limit 覆盖），`-1` 不再允许无限重连。
- helper 职责收窄：helper 仅执行 `start`（启动 VPN + 特权操作）和 `stop`（清理），不参与重连决策。

**Tech Stack:** C++17, Windows named pipe IPC, nlohmann/json

---

## 当前问题分析

### 问题 1：僵尸 helper 进程
- `run_native_supervisor_impl` 中 `retry_limit=-1` 时无限重连
- supervisor 进程在 VPN 断开后持续重试，reconnect attempt 计数达到 2000+
- 每次重连失败都会创建新的 NativeSessionEventRecorder，写入新的 session state 文件

### 问题 2：oneshot helper 不自动退出
- oneshot helper 只在收到 "stop" action 或 worker 初次启动失败时退出
- 如果 worker 成功启动但 VPN 后来断开，supervisor 进程无限重连，helper 进程不退出
- core 挂掉后 helper 永远不会知道

### 问题 3：无心跳机制
- helper 和 core 之间没有健康检查
- core 无法通知 helper "我不再需要你了"
- helper 无法检测 core 是否存活

---

### Task 1: 限制 supervisor 重连上限

**Files:**
- Modify: `src/vpn.cpp:585-601` (native supervisor retry loop)
- Modify: `src/vpn.cpp:780-802` (legacy supervisor retry loop)

**目标:** 将 `retry_limit=-1`（无限重连）改为默认上限 3 次。`retry_limit=-1` 在 IPC 协议中不再被接受。

- [ ] **Step 1: 修改 `run_native_supervisor_impl` 的重连逻辑**

在 `src/vpn.cpp:585-601`，将无限重连模式替换为有上限的重连：

```cpp
// 当前代码 (line 585-601):
while (!supervisor_stop_requested) {
    if (!first_attempt) {
      if (retry_limit == 0)
        break;
      if (retry_limit > -1 && reconnect_attempts_used >= retry_limit) {
        logger::warn("Native VPN supervisor reached retry limit: " +
                     std::to_string(retry_limit));
        break;
      }
      ++reconnect_attempts_used;
      logger::warn("Native VPN session ended; reconnect attempt " +
                   std::to_string(reconnect_attempts_used) +
                   (retry_limit > -1 ? ("/" + std::to_string(retry_limit))
                                     : " (infinite mode)"));
      sleep_ms(2000);
```

修改为：

```cpp
constexpr int kDefaultMaxReconnectAttempts = 3;

while (!supervisor_stop_requested) {
    if (!first_attempt) {
      const int effective_limit =
          (retry_limit < 0) ? kDefaultMaxReconnectAttempts : retry_limit;
      if (effective_limit == 0)
        break;
      if (reconnect_attempts_used >= effective_limit) {
        logger::warn("Native VPN supervisor reached retry limit: " +
                     std::to_string(effective_limit));
        break;
      }
      ++reconnect_attempts_used;
      logger::warn("Native VPN session ended; reconnect attempt " +
                   std::to_string(reconnect_attempts_used) +
                   "/" + std::to_string(effective_limit));
      sleep_ms(2000);
```

- [ ] **Step 2: 同样修改 `run_supervisor` 的重连逻辑**

在 `src/vpn.cpp:780-802`，应用相同的修改（legacy supervisor 路径）。

- [ ] **Step 3: 构建并运行相关测试**

```bash
cd build-win-codex && ninja -j4
```

- [ ] **Step 4: Commit**

```bash
git add src/vpn.cpp
git commit -m "fix(vpn): cap supervisor reconnect at 3 attempts when retry_limit is infinite"
```

---

### Task 2: 在 IPC 协议中添加心跳 action

**Files:**
- Modify: `src/helper.cpp` (handle_request dispatch, 新增 handle_heartbeat)
- Modify: `src/helper_ipc.hpp` (如果需要定义新的 action 常量)

**目标:** 添加 `heartbeat` action，core 定期发送心跳，helper 返回确认。helper 在心跳超时后自销毁。

- [ ] **Step 1: 在 `handle_request` 中添加 heartbeat action 分发**

在 `src/helper.cpp:1016-1022` 的 `handle_request` 函数中，添加 heartbeat action：

```cpp
// 当前代码:
if (action == "hello")
    return make_hello_response();
if (action == "start")
    return handle_start(peer_identity, request);
if (action == "stop")
    return handle_stop(peer_identity, request);
if (action == "status")
    ...
```

在 `"hello"` 之后添加：

```cpp
if (action == "heartbeat")
    return handle_heartbeat(peer_identity, request);
```

- [ ] **Step 2: 实现 `handle_heartbeat` 函数**

在 `handle_request` 函数之前添加：

```cpp
nlohmann::json handle_heartbeat(const internal::PeerIdentity & /*peer_identity*/,
                                const nlohmann::json & /*request*/) {
  // 更新最后心跳时间戳
  last_heartbeat_time = std::chrono::steady_clock::now();
  heartbeat_received = true;

  // 返回 helper 当前状态
  SessionState state;
  bool has_session = load_session_state(&state);
  return nlohmann::json{{"ok", true},
                        {"has_session", has_session},
                        {"pid", static_cast<int>(getpid())}};
}
```

- [ ] **Step 3: 添加心跳状态变量**

在 `src/helper.cpp` 的匿名命名空间中（约 line 48），添加：

```cpp
std::chrono::steady_clock::time_point last_heartbeat_time =
    std::chrono::steady_clock::now();
bool heartbeat_received = false;
constexpr auto kHeartbeatTimeout = std::chrono::seconds(30);
```

- [ ] **Step 4: 构建验证**

```bash
cd build-win-codex && ninja -j4
```

- [ ] **Step 5: Commit**

```bash
git add src/helper.cpp
git commit -m "feat(helper): add heartbeat IPC action for liveness detection"
```

---

### Task 3: 在 daemon_main 循环中集成心跳超时自销毁

**Files:**
- Modify: `src/helper.cpp:1511-1549` (daemon_main 的主循环)

**目标:** oneshot helper 在主循环中检测心跳超时，超时后自动退出。

- [ ] **Step 1: 修改 daemon_main 的主循环**

在 `src/helper.cpp:1511-1549` 的 while 循环中，添加心跳超时检测：

```cpp
// 当前代码:
while (!daemon_stop_requested) {
    reap_finished_request_handlers();

    if (!ipc->accept_client()) {
      if (daemon_stop_requested)
        break;
      continue;
    }
    ...
```

修改为：

```cpp
while (!daemon_stop_requested) {
    reap_finished_request_handlers();

    // 心跳超时检测：仅 oneshot 模式，且已收到过心跳
    if (active_daemon_options.oneshot && heartbeat_received) {
      auto now = std::chrono::steady_clock::now();
      if (now - last_heartbeat_time > kHeartbeatTimeout) {
        logger::warn("[helper-lifecycle] stage=heartbeat_timeout "
                     "reason=core_heartbeat_expired timeout_s=" +
                     std::to_string(kHeartbeatTimeout.count()));
        daemon_stop_requested = 1;
        break;
      }
    }

    if (!ipc->accept_client()) {
      if (daemon_stop_requested)
        break;
      continue;
    }
    ...
```

- [ ] **Step 2: 构建验证**

```bash
cd build-win-codex && ninja -j4
```

- [ ] **Step 3: Commit**

```bash
git add src/helper.cpp
git commit -m "feat(helper): auto-destroy oneshot helper on heartbeat timeout"
```

---

### Task 4: core 侧发送心跳

**Files:**
- Modify: `src/platform/win32/helper_lifecycle.cpp` (或 helper_client 相关文件)
- 查找: core 侧启动 helper 后的等待/轮询逻辑

**目标:** core 在 helper 存续期间定期发送心跳。

- [ ] **Step 1: 定位 core 侧 helper 管理代码**

查找 core 如何启动 helper 以及在哪里等待 helper 响应。

- [ ] **Step 2: 添加心跳发送逻辑**

在 core 启动 helper 之后，启动一个后台线程定期发送心跳：

```cpp
// 在 core 的 helper 管理模块中
void start_heartbeat_thread() {
  std::thread([] {
    while (helper_alive) {
      std::this_thread::sleep_for(std::chrono::seconds(10));
      auto response = send_helper_request({{"action", "heartbeat"}});
      if (!response.value("ok", false)) {
        // helper 不可达，停止心跳
        break;
      }
    }
  }).detach();
}
```

- [ ] **Step 3: 在 helper 停止时停止心跳**

- [ ] **Step 4: 构建验证**

- [ ] **Step 5: Commit**

---

### Task 5: 添加集成测试

**Files:**
- Create: `tests/helper_heartbeat_test.cpp` (或在现有测试文件中添加)
- Modify: `CMakeLists.txt` (添加新测试目标)

**目标:** 验证心跳超时机制和重连上限。

- [ ] **Step 1: 编写心跳超时测试**

测试场景：
1. 启动 oneshot helper
2. 发送一次心跳
3. 等待超过超时时间
4. 验证 helper 自动退出

- [ ] **Step 2: 编写重连上限测试**

测试场景：
1. 启动 supervisor，retry_limit=-1
2. 模拟连接失败
3. 验证重连次数不超过 3 次

- [ ] **Step 3: 运行测试**

- [ ] **Step 4: Commit**

---

### Task 6: 清理现有僵尸进程

**Files:**
- Modify: `src/helper.cpp` (添加清理逻辑)
- Modify: `src/platform/win32/helper_lifecycle.cpp`

**目标:** 在 helper 启动时清理残留的 supervisor 进程。

- [ ] **Step 1: 在 helper 启动时扫描并清理僵尸 supervisor**

在 `daemon_main` 开始时，扫描并终止残留的 `__vpn-supervisor` 进程。

- [ ] **Step 2: 构建验证**

- [ ] **Step 3: Commit**
