# ECNU-VPN 架构整改方案与 5 子智能体并行开发计划

> 版本：Draft 1  
> 日期：2026-06-02  
> 输入依据：`PROJECT_CURRENT_ARCHITECTURE.md` 当前架构审计文档。  
> 目标：把当前“UI / exv CLI / helper daemon / worker / supervisor / native engine”混杂模型，逐步改造成“UI 只交互、Core 管连接生命周期和协议、Helper 只做特权系统操作”的可维护架构，同时保留 transient helper 与 resident helper 两种体验。

---

## 0. 核心判断

当前项目已经有两个非常好的基础：

1. **协议层已经 clean-room 自研，并且协议层本身不依赖 UI、helper 或平台 API。** 这说明核心 AnyConnect 协议可以被提升为真正的 `corelib`，而不是继续嵌在 helper worker/supervisor 的历史流程中。
2. **helper 已经支持 service / foreground / oneshot 多模式。** 这说明 transient helper 与 resident helper 不需要重写，只需要把“helper 做什么”和“helper 如何存活”解耦。

当前最大的问题不是“缺少模块”，而是**生命周期 owner 错位**：

- 当前 `__vpn-supervisor` 是重连 owner；
- 当前 `exv-helper` 是特权 IPC owner，同时还间接成为 VPN 会话 owner；
- 当前 `vpn.cpp` 同时承载 legacy supervisor、native supervisor、start/stop、状态报告；
- 当前高权限 worker/supervisor 仍会运行 native 协议栈，导致远端输入解析出现在 SYSTEM/root 权限链路中。

目标架构是把这些职责拉直：

```text
UI / Electron / WebView / CLI
  - 展示状态
  - 收集用户输入
  - 发出 connect/disconnect/setAutoReconnect 等意图
  - 不决定 retry_limit
  - 不直接执行特权网络操作

Core / TunnelController
  - 唯一连接生命周期 owner
  - 持有 UserIntent
  - 持有 TunnelState
  - 执行 AnyConnect 认证、CSTP/TLS 通信、packet pump、断线重连
  - 决定是否重连、何时重连、是否清理
  - 调 helper 做特权控制面操作

Privileged Helper
  - 只做白名单特权操作
  - 创建/打开/准备虚拟网卡
  - 应用/清理 IP、route、DNS、防火墙/kill switch
  - 维护 session lease / heartbeat / cleanup registry
  - 不连接 VPN 服务器
  - 不解析服务器响应
  - 不转发每个 packet
  - transient/resident 共用同一套 API，只改变 LifecyclePolicy
```

---

## 1. 当前架构要点与整改驱动力

### 1.1 当前进程模型

审计文档显示，当前运行时包含：`exv`、`exv-helper`、`exv-helper __helper-exec`、`__vpn-supervisor`、Electron desktop。`exv-helper` 是特权守护进程，`__helper-exec` 是请求级 worker，`__vpn-supervisor` 是 VPN 连接级重连 supervisor。

这带来的直接后果是：Native VPN 连接生命周期跨越 `app_api.cpp`、`helper.cpp`、`vpn.cpp`、`native_engine` 和 supervisor 子进程，不存在单一 owner。

### 1.2 当前已经满足的目标

当前项目并不是推倒重来。它已经满足或基本满足这些目标：

- 协议层 clean-room 自研；
- 协议层本身不依赖 UI/helper/platform；
- helper 不在数据面；
- 数据面没有跨进程 IPC；
- helper 已经有 service/foreground/oneshot 多模式；
- oneshot helper 已有心跳超时退出；
- resident helper 已由 Windows SCM / macOS launchd / Linux systemd 管理。

### 1.3 当前必须整改的问题

最高优先级问题：

- 重连策略分散，没有统一状态机；
- `vpn.cpp`、`helper.cpp`、`app_api.cpp` 过大且职责混杂；
- 高权限 worker/supervisor 会运行协议栈并解析远端输入；
- helper 会话状态、IPC、worker/supervisor 编排混在一个大文件里；
- Windows 路由清理缺失；
- 密钥没有 DPAPI/Keychain 集成；
- CI 没有运行 CTest；
- Linux native 路径不完整；
- macOS 没有 Network Extension。

---

## 2. 目标运行时布局

### 2.1 默认模式：不安装服务，一次性提权

```text
Electron/WebView/CLI UI
  ⇅ user IPC / desktop-rpc
Core process: exv core mode, normal user
  ⇅ helper RPC
exv-helper --oneshot / --mode=transient, elevated
```

特点：

- 用户不需要预装 helper；
- 每次连接需要 UAC / sudo / pkexec；
- transient helper 在 core 心跳消失后 cleanup 并退出；
- core 负责断线重连；
- 若处于 Reconnecting 且用户允许 auto_reconnect，core 继续给 helper heartbeat，helper 不应立即退出；
- 若用户主动 disconnect 或 auto_reconnect=false 且连接失败，core 调用 cleanup，helper idle 后退出。

### 2.2 增强模式：安装 resident helper，但 core 仍是用户进程

```text
Electron/WebView/CLI UI
  ⇅
Core process, normal user
  ⇅
exv-helper --service, elevated, resident or demand-start
```

特点：

- 安装 helper 后连接不再反复提权；
- helper 崩溃由 OS 管理；
- 协议栈仍运行在普通用户 core 中；
- helper 只做特权控制面；
- core 崩溃后 helper 根据 lease 超时清理。

这是正式版推荐默认安装体验。

### 2.3 企业/高级模式：Core Service + Resident Helper

```text
UI
  ⇅
Core Service
  ⇅
Resident Helper
```

只为 always-on、登录前 VPN、企业部署预留。校园轻量客户端不应把它作为默认路线，因为网页登录、MFA、UI 会话通信会变复杂。

### 2.4 macOS 目标模式

短期可以沿用当前 utun + helper 模式；中长期应抽象出：

```text
MyVpn.app
  ⇅
PacketTunnelProvider.appex
  - 链接 corelib
  - 执行协议、packetFlow、重连
  - 应用 NE tunnel settings

Optional privileged helper
  - 仅做安装、迁移、诊断、特殊清理
```

不要为了“跨平台进程数一致”而牺牲 macOS 原生 Network Extension 模型。

---

## 3. 新模块布局

建议逐步改成以下结构。名称可以按项目习惯微调，但边界不要变。

```text
src/
  core/
    tunnel_controller.hpp/.cpp
    tunnel_intent.hpp
    tunnel_state.hpp
    tunnel_events.hpp
    reconnect_policy.hpp/.cpp
    reconnect_state_machine.hpp/.cpp
    tunnel_session.hpp/.cpp
    core_status_snapshot.hpp
    core_error_mapper.hpp/.cpp
    timing.hpp/.cpp

  core_api/
    app_rpc_dispatcher.hpp/.cpp
    vpn_actions.hpp/.cpp
    config_actions.hpp/.cpp
    service_actions.hpp/.cpp
    route_actions.hpp/.cpp

  helper_common/
    helper_protocol.hpp
    helper_messages.hpp/.cpp
    helper_capabilities.hpp
    helper_error.hpp
    helper_client.hpp/.cpp
    helper_connector.hpp
    helper_session_lease.hpp

  helper_runtime/
    helper_server.hpp/.cpp
    helper_request_dispatcher.hpp/.cpp
    helper_lifecycle_policy.hpp/.cpp
    session_lease_manager.hpp/.cpp
    cleanup_registry.hpp/.cpp
    helper_session_store.hpp/.cpp
    command_validator.hpp/.cpp

  platform/
    common/
      platform_network_ops.hpp
      tunnel_device_descriptor.hpp
      packet_device_factory.hpp
      route_model.hpp
      dns_model.hpp
      credential_store.hpp
      process_supervision.hpp
      ipc_transport.hpp
    win32/
      win_network_ops.cpp
      win_wintun_device.cpp
      win_route_ops.cpp
      win_dns_ops.cpp
      win_firewall_ops.cpp
      win_helper_connector.cpp
      win_named_pipe_transport.cpp
      win_credential_store.cpp
    darwin/
      darwin_network_ops.mm
      darwin_utun_device.cpp
      darwin_route_ops.cpp
      darwin_dns_ops.mm
      darwin_helper_connector.mm
      darwin_xpc_or_socket_transport.mm
      darwin_keychain_store.mm
      packet_tunnel_provider/   # later milestone
    linux/
      linux_network_ops.cpp
      linux_tun_device.cpp
      linux_route_ops.cpp
      linux_dns_ops.cpp
      linux_helper_connector.cpp
      linux_credential_store.cpp

  vpn_engine/
    protocol/                  # 尽量保持纯协议，不依赖 core/helper/platform
    native_engine.*             # 改造成被 TunnelController 驱动的 engine/session
    packet_device.hpp           # 保留抽象，但去掉“顺手配置 route/DNS”的职责
```

### 3.1 旧文件去向

| 当前文件 | 问题 | 目标去向 |
|---|---|---|
| `src/vpn.cpp` | 混合 native/legacy supervisor、start/stop/status | 拆成 `core/tunnel_controller`, `core/reconnect_policy`, `legacy/legacy_openconnect_adapter`, `core/native_session_runner` |
| `src/helper.cpp` | 混合 IPC、daemon 生命周期、session store、worker 入口 | 拆成 `helper_runtime/*` 与 `helper_common/*` |
| `src/app_api.cpp` | 单一巨型 RPC 分发函数 | 拆成 `core_api/*_actions` 和纯 dispatcher |
| `src/tunnel.cpp` | 脚本式路由配置 | 长期迁移到 `platform/*_route_ops`，保留 legacy adapter |
| `src/helper_lifecycle.cpp` | worker/supervisor 编排过重 | 只保留 helper runtime 进程生命周期；VPN lifecycle 移到 core |
| `src/platform/common/vpn_supervisor_process.hpp` | supervisor payload 跨进程边界 | native 路径废弃；legacy 路径隔离在 legacy adapter |

---

## 4. 新核心状态机

### 4.1 UserIntent

```cpp
struct UserIntent {
    bool desired_connected = false;
    bool auto_reconnect = true;
    ProfileId profile_id;
    std::optional<DisconnectReason> user_disconnect_reason;
};
```

语义：

- `desired_connected=false`：用户明确不希望 VPN 保持连接，任何断开都不重连；
- `auto_reconnect=false`：意外断开后不重连，但用户仍可以手动点 Connect；
- `auto_reconnect=true`：recoverable failure 进入 Reconnecting。

### 4.2 RuntimeState

```cpp
enum class TunnelPhase {
    Idle,
    PreparingHelper,
    Authenticating,
    ConnectingCstp,
    ApplyingNetworkConfig,
    OpeningPacketDevice,
    Connected,
    Reconnecting,
    Disconnecting,
    CleaningUp,
    Failed
};
```

### 4.3 事件输入

```cpp
enum class TunnelEventType {
    UserConnect,
    UserDisconnect,
    SetAutoReconnect,
    HelperReady,
    AuthSucceeded,
    AuthFailed,
    CstpConnected,
    NetworkConfigApplied,
    PacketLoopStarted,
    TransportClosed,
    PacketDeviceFailed,
    HelperLost,
    LeaseExpired,
    ReconnectTimerFired,
    CleanupSucceeded,
    CleanupFailed
};
```

### 4.4 重连决策

`ReconnectPolicy` 只接收结构化错误和用户意图：

```cpp
struct ReconnectDecision {
    bool should_retry;
    std::chrono::milliseconds delay;
    std::string reason_code;
    bool keep_helper_session;
    bool keep_network_config;
};
```

规则：

| 场景 | auto_reconnect | 动作 |
|---|---:|---|
| 用户主动断开 | 任意 | 不重连，cleanup |
| auth failed / bad credential | true/false | 不重连，Failed(auth) |
| cert error | true/false | 不重连，Failed(cert) |
| transport_closed after stable_ready | true | Reconnecting，指数退避 |
| transport_closed after stable_ready | false | Failed，cleanup |
| packet device / route apply failure | true/false | 不盲目重连协议，先报 OS config error |
| helper lost during Connected | true | 尝试 reconnect helper / reconcile；失败再 Failed |
| helper lost during ApplyingNetworkConfig | 任意 | Failed(helper_unavailable) |

### 4.5 退避策略

建议：

```text
base = 1s
max = 60s
jitter = ±20%
reset condition = connected stable 60s
```

---

## 5. Helper 新协议

### 5.1 原则

Helper API 只表达“特权控制面”，不表达“启动 VPN 协议”。

废弃 native 路径中的：

```json
{ "action": "start", "auth_session": ..., "retry_limit": ... }
```

新增：

```json
{ "op": "Hello", "protocol_version": 2 }
{ "op": "StartSession", "profile_id": "...", "mode": "transient|resident" }
{ "op": "PrepareTunnelDevice", "session_id": "...", "request": {...} }
{ "op": "ApplyTunnelConfig", "session_id": "...", "config": {...} }
{ "op": "Heartbeat", "session_id": "...", "state": "Connected|Reconnecting|..." }
{ "op": "Cleanup", "session_id": "...", "policy": {...} }
{ "op": "GetSnapshot" }
{ "op": "EndSession", "session_id": "..." }
```

Legacy openconnect 可以继续临时用旧 `start/stop`，但必须封装为 `legacy_openconnect_adapter`，不要让旧 action 继续污染 native 目标架构。

### 5.2 Session lease

```cpp
struct HelperSessionLease {
    SessionId session_id;
    ProfileId profile_id;
    HelperMode mode; // Transient / Resident
    std::chrono::steady_clock::time_point last_heartbeat;
    TunnelPhase core_phase;
    CleanupPolicy cleanup_policy;
};
```

生命周期：

```text
StartSession
  → PrepareTunnelDevice
  → ApplyTunnelConfig
  → Heartbeat loop
  → Cleanup
  → EndSession
```

超时策略：

| Helper mode | 心跳超时 | cleanup 后是否退出 |
|---|---|---|
| Transient | cleanup session | idle_timeout 后退出 |
| Resident | cleanup session | 继续待命 |

Reconnecting 期间：

- core 必须继续 heartbeat；
- helper 不应因 TLS 断开而 cleanup；
- 超过 `max_reconnect_window` 后由 core 决定 cleanup。

### 5.3 Cleanup registry

Helper 必须记录自己创建/修改过的对象：

```json
{
  "session_id": "...",
  "adapter": "...",
  "addresses": [...],
  "routes": [...],
  "dns": {...},
  "firewall_rules": [...],
  "created_at": "..."
}
```

要求：

- cleanup 幂等；
- cleanup 支持崩溃后恢复；
- cleanup 不依赖 core 正常退出；
- Windows route cleanup 不允许继续 no-op；
- helper 启动时扫描 stale session 并按策略清理。

### 5.4 Helper 安全边界

禁止：

- `RunCommand`；
- shell 字符串透传；
- helper 解析 AnyConnect XML/HTML；
- helper 接收用户名密码；
- helper 持有 webvpn_session cookie；
- helper 转发每个 IP packet。

允许：

- 根据 core 提供的结构化 `TunnelConfig` 写入 OS 网络栈；
- 根据 session registry 清理自己创建的 route/DNS/adapter；
- 验证调用方身份；
- 版本协商与能力协商。

---

## 6. 数据面和控制面拆分

当前 `PacketDevice::open(metadata)` 里包含“打开设备 + 配置网络”的混合职责。目标应拆成：

```cpp
class PacketDevice {
public:
    virtual Result<void> open(const TunnelDeviceDescriptor&) = 0;
    virtual Result<Packet> read_packet() = 0;
    virtual Result<void> write_packet(Packet) = 0;
    virtual void close() = 0;
};

class PlatformNetworkOps {
public:
    virtual Result<TunnelDeviceDescriptor> prepare_tunnel_device(SessionId, TunnelDeviceRequest) = 0;
    virtual Result<void> apply_tunnel_config(SessionId, TunnelConfig) = 0;
    virtual Result<void> cleanup(SessionId, CleanupPolicy) = 0;
};
```

控制面：

```text
Core → Helper → PlatformNetworkOps → OS routes/DNS/adapter
```

数据面：

```text
Core PacketDevice ↔ OS tunnel device ↔ Core ProtocolEngine ↔ AnyConnect server
```

Helper 不进入数据面。

### 6.1 Windows 注意点

必须尽早验证：普通用户 core 是否能打开/读写 helper 创建的 Wintun adapter/session。

如果不行，候选方案：

1. helper 创建 adapter 并设置 ACL，使 core 可打开数据面；
2. helper 创建/打开后将必要 handle 安全传给 core；
3. 短期保留高权限 core adapter 逻辑，但协议解析尽快降权；
4. 最不推荐：helper 参与 packet 转发。

### 6.2 macOS 注意点

短期可继续 utun + helper；长期 PacketTunnelProvider 直接拥有 packetFlow，不需要 Windows 式 helper 来创建 TUN。

### 6.3 Linux 注意点

Linux native 缺少 `/dev/net/tun` 实现；如果 Linux 仍在范围内，应该由平台 Agent 补齐，否则显式标记为 legacy-only。

---

## 7. UI / RPC / 配置层整改

### 7.1 UI 不再计算 retry_limit

当前 UI/API 把 `auto_reconnect` 翻译成 `retry_limit` 传给 helper。目标：UI 只传用户意图。

```json
{
  "action": "vpn.connect",
  "profile_id": "default",
  "intent": {
    "desired_connected": true,
    "auto_reconnect": true
  }
}
```

Core 负责读取 config 与当前 UI intent，决定 `ReconnectPolicy`。

### 7.2 `app_api.cpp` 拆分

目标：

```text
core_api/app_rpc_dispatcher.cpp
  - 只做 action → handler 分发

core_api/vpn_actions.cpp
  - connect/disconnect/status

core_api/config_actions.cpp
  - config.get/save

core_api/service_actions.cpp
  - service/helper/driver status/install

core_api/route_actions.cpp
  - user route management
```

### 7.3 状态 API

Core 应统一输出：

```json
{
  "phase": "Connected|Reconnecting|Failed|...",
  "desired_connected": true,
  "auto_reconnect": true,
  "helper_mode": "transient|resident",
  "helper_status": "connected|unavailable|version_mismatch",
  "network_ready": true,
  "server": "...",
  "interface": "...",
  "last_error": {
    "domain": "transport|auth|helper|os.route|os.dns|packet",
    "code": "transport_closed",
    "recoverable": true,
    "native_code": 0,
    "recommended_action": "..."
  },
  "reconnect": {
    "attempt": 2,
    "next_retry_ms": 4000
  }
}
```

---

## 8. 错误、日志、凭据、安全整改

### 8.1 错误模型

统一错误类型：

```cpp
struct ErrorInfo {
    std::string domain;
    std::string code;
    std::string message;
    std::optional<int> native_code;
    std::string native_api;
    bool recoverable;
    std::string recommended_action;
};
```

所有 helper/protocol/platform 错误都转成这个结构，而不是 `make_error("string")`。

### 8.2 凭据

优先级：

1. Windows 使用 DPAPI / Credential Manager 存储 AES key 或直接存 token/password；
2. macOS 使用 Keychain；
3. Linux 使用 Secret Service/libsecret，如果不可用则降级为文件并强提示；
4. 不再通过命令行参数传递 auth_token/password；
5. 不把含明文密码的请求文件落盘；如必须落盘，使用 OS 安全临时文件 + 立即清零 + 审计测试。

### 8.3 高权限远端输入解析

目标验收：

- SYSTEM/root helper 不再调用 `protocol/auth.cpp`；
- SYSTEM/root helper 不再持有 VPN server response；
- fuzz auth/http/xml parser；
- parser 崩溃不会变成提权路径。

---

## 9. 测试与 CI

最低要求：

- CI 运行所有可运行的 CTest；
- 新增 `ReconnectPolicy` 单元测试；
- 新增 `TunnelController` 状态机测试；
- 新增 fake helper 测试；
- 新增 helper lease timeout 测试；
- 新增 Windows route cleanup 测试；
- 新增 helper protocol version mismatch 测试；
- 新增 UI contract snapshot 测试；
- 新增 security regression：命令行不出现 password/auth_token。

---

## 10. 分阶段迁移路线

### Phase 0：安全基线与契约冻结

目标：为 5 个智能体并行提供共同接口，避免互相覆盖。

产物：

- `docs/ARCHITECTURE_TARGET.md`
- `docs/HELPER_PROTOCOL_V2.md`
- `docs/CORE_STATE_MACHINE.md`
- `src/core/*` 接口骨架
- `src/helper_common/*` 接口骨架
- `src/platform/common/*` 接口骨架
- CI 至少能 build 原有项目

不改默认运行路径。

### Phase 1：提取低风险共享模块

- 提取 `StageTimer` / `ConnectTiming`；
- 提取 `NativeStartupFailureState`；
- 提取统一错误类型；
- 拆 `app_api.cpp` dispatcher；
- 拆 `helper.cpp` session store / lease manager；
- 保持功能行为不变。

### Phase 2：引入 Core `TunnelController`

- 实现状态机；
- 实现 `ReconnectPolicy`；
- native 路径先可在 shadow mode 运行：只记录决策，不接管实际连接；
- 测试覆盖所有状态转换。

### Phase 3：Helper Protocol V2 并行上线

- 旧 `start/stop/status` 继续支持 legacy；
- 新 `StartSession/ApplyTunnelConfig/Heartbeat/Cleanup` 上线；
- transient/resident 使用同一套协议；
- helper 不再新增任何协议层依赖。

### Phase 4：控制面迁移到 helper V2

- `PacketDevice::open` 拆分；
- helper 负责 apply/cleanup；
- core 负责协议和 packet pump；
- Windows route cleanup 修复；
- helper cleanup registry 实现。

### Phase 5：native path 切换到 Core-owned lifecycle

- native 不再使用 `__vpn-supervisor` 作为重连 owner；
- native 不再用 helper worker 跑协议；
- auto reconnect 由 `TunnelController` 控制；
- legacy openconnect supervisor 留作隔离 adapter。

### Phase 6：平台完善与安全加固

- DPAPI/Keychain；
- Linux socket 权限修复；
- route policy validation；
- Network Extension PoC；
- CI full CTest；
- 删除废弃入口或加 deprecated guard。

---

# 11. 5 个智能体并行工作计划

## 协作规则

### 共享规则

1. 每个智能体只改自己负责的目录。
2. 修改共享接口必须先改 `docs/INTERFACE_CHANGELOG.md`，并通知其他智能体。
3. 大文件拆分优先于大规模逻辑改写。
4. 所有新增接口必须有单元测试或 fake 实现。
5. 原有功能必须以 feature flag 保留，直到集成 Agent 通过 E2E。

### 分支建议

```text
agent-1-core-controller
agent-2-helper-runtime
agent-3-platform-network-ops
agent-4-ui-api-config-security
agent-5-tests-ci-integration
```

### 共享接口归属

| 文件/目录 | Owner | 其他 Agent 权限 |
|---|---|---|
| `src/core/*` | Agent 1 | 只读；改动需 PR 到 Agent 1 |
| `src/helper_common/*` | Agent 2 | 可提 issue，不直接改 |
| `src/platform/common/*` | Agent 3 | 可提 issue，不直接改 |
| `webui/desktop/shared/desktop-contract.ts` | Agent 4 | Agent 5 可加测试 |
| `tests/support/*` | Agent 5 | 其他 Agent 可添加小 fake，但需同步 |

---

## Agent 1：Core Lifecycle / Reconnect State Machine

### 目标

建立 `TunnelController`，让 core 成为连接生命周期和重连策略的唯一 owner。

### 负责范围

```text
src/core/
src/vpn_engine/session_state.*
src/vpn_engine/native_engine.* 的接口适配
src/vpn.cpp 中 native supervisor 逻辑的逐步迁移
```

### 主要任务

1. 新建 `UserIntent`, `TunnelPhase`, `TunnelEvent`, `TunnelStatusSnapshot`。
2. 新建 `ReconnectPolicy`，支持 auto_reconnect 开关、recoverable 分类、指数退避、用户主动断开识别。
3. 新建 `TunnelController`，统一 connect/disconnect/reconnect/cleanup 流程。
4. 把当前 `retry_limit` 翻译逻辑从 UI/app_api 下沉到 core。
5. 引入 shadow mode：现有 supervisor 仍运行，但 core 状态机记录预期决策，用日志对比。
6. 抽出 `StageTimer` / `ConnectTiming` 共享实现。
7. 抽出 `NativeStartupFailureState` 到统一模块。

### 不允许做

- 不改 helper IPC 传输实现；
- 不改 Electron UI；
- 不直接改平台 route/DNS 实现；
- 不删除 legacy openconnect 路径。

### 交付物

- `src/core/tunnel_controller.hpp/.cpp`
- `src/core/reconnect_policy.hpp/.cpp`
- `src/core/tunnel_state.hpp`
- `src/core/tunnel_intent.hpp`
- `src/core/tunnel_events.hpp`
- `src/core/timing.hpp/.cpp`
- `src/vpn_engine/native_startup_failure.hpp/.cpp`
- `tests/reconnect_policy_test.cpp`
- `tests/tunnel_controller_state_machine_test.cpp`
- `docs/CORE_STATE_MACHINE.md`

### 验收标准

- `auto_reconnect=false` 时，transport_closed 不进入 Reconnecting；
- 用户主动 disconnect 不重连；
- auth/cert/credential 错误不重连；
- stable connected 后 transport_closed 且 auto_reconnect=true 进入 Reconnecting；
- backoff 有上限和 jitter，可测试地注入 fake clock；
- CTest 中新增状态机测试全部通过；
- 原有 native/legacy build 不破坏；
- `vpn.cpp` 中新增逻辑不超过薄适配，核心决策在 `src/core/`。

---

## Agent 2：Helper Protocol / Runtime / Lease / Cleanup

### 目标

把 helper 变成“同一套特权操作 API + 两种生命周期策略”的最小权限组件。

### 负责范围

```text
src/helper.cpp 拆分
src/helper_main.cpp
src/helper_common/
src/helper_runtime/
src/helper_ipc.hpp 兼容演进
helper service manager 仅限生命周期接口
```

### 主要任务

1. 定义 Helper Protocol V2：Hello, StartSession, PrepareTunnelDevice, ApplyTunnelConfig, Heartbeat, Cleanup, GetSnapshot, EndSession。
2. 保留旧 `start/stop/status/heartbeat/hello`，但标记为 LegacyHelperProtocol。
3. 新建 `SessionLeaseManager`，支持 transient/resident 生命周期策略。
4. 新建 `CleanupRegistry`，记录 adapter/routes/dns/firewall 资源。
5. 新建 `HelperLifecyclePolicy`，把 `--oneshot` 与 `--service` 差异收敛成配置。
6. 新建版本协商和 capabilities。
7. helper 启动时扫描 stale session 并清理。
8. 禁止新增任何协议层依赖；helper 不 include `vpn_engine/protocol/*`。

### 不允许做

- 不实现具体 Windows route/DNS 逻辑；
- 不改 UI；
- 不运行协议连接；
- 不把密码/cookie/token 放入 helper V2 消息。

### 交付物

- `src/helper_common/helper_messages.hpp/.cpp`
- `src/helper_common/helper_protocol.hpp`
- `src/helper_common/helper_capabilities.hpp`
- `src/helper_runtime/helper_server.hpp/.cpp`
- `src/helper_runtime/helper_request_dispatcher.hpp/.cpp`
- `src/helper_runtime/session_lease_manager.hpp/.cpp`
- `src/helper_runtime/cleanup_registry.hpp/.cpp`
- `src/helper_runtime/helper_lifecycle_policy.hpp/.cpp`
- `docs/HELPER_PROTOCOL_V2.md`
- `tests/helper_protocol_test.cpp`
- `tests/helper_lease_manager_test.cpp`

### 验收标准

- transient helper 在 heartbeat 超时后 cleanup 并退出；
- resident helper 在 heartbeat 超时后 cleanup 但不退出；
- `Hello` 返回 protocol_version 与 capabilities；
- unknown op 返回结构化错误；
- helper V2 消息 schema 不包含 username/password/cookie；
- helper 代码不 include `protocol/auth.hpp`、`protocol/session.hpp`；
- legacy `start/stop/status` 仍能编译并通过冒烟测试；
- cleanup registry 操作幂等。

---

## Agent 3：Platform Network Ops / Packet Device / Route-DNS Cleanup

### 目标

把平台网络操作从 native engine/session 中抽出，形成统一 `PlatformNetworkOps`，并修复路由清理与平台隔离问题。

### 负责范围

```text
src/platform/common/
src/platform/win32/
src/platform/darwin/
src/platform/linux/ 视范围决定
src/vpn_engine/packet_device.hpp 的拆分适配
```

### 主要任务

1. 定义平台无关 `TunnelConfig`, `Route`, `DnsConfig`, `TunnelDeviceDescriptor`, `CleanupPolicy`。
2. 拆分 `PacketDevice::open(metadata)` 中的网络配置职责。
3. Windows：实现 prepare/apply/cleanup，修复 route cleanup no-op。
4. macOS：把 utun/route/DNS 操作适配到相同模型。
5. Linux：若本轮纳入，新增 `/dev/net/tun` PacketDevice；否则显式返回 unsupported，并写测试。
6. 提供 fake platform ops 供 Agent 1/5 测试。
7. 避免 shell 字符串拼接；必须使用结构化系统 API 或严格参数数组。

### 不允许做

- 不改 helper protocol 语义；
- 不改 UI；
- 不改 AnyConnect 协议层；
- 不删除 tunnel script legacy 路径。

### 交付物

- `src/platform/common/platform_network_ops.hpp`
- `src/platform/common/tunnel_config.hpp`
- `src/platform/common/route_model.hpp`
- `src/platform/common/dns_model.hpp`
- `src/platform/common/packet_device_factory.hpp`
- `src/platform/win32/win_network_ops.cpp`
- `src/platform/darwin/darwin_network_ops.mm/.cpp`
- `src/platform/linux/linux_network_ops.cpp` 或 unsupported stub
- `tests/platform_network_ops_model_test.cpp`
- `tests/win_route_cleanup_test.cpp` 或平台条件测试

### 验收标准

- Windows apply 后 cleanup 可移除本 session 创建的 routes；
- cleanup 幂等，重复执行不报错；
- 不再由 `PacketDevice::open` 直接承担 route/DNS 全部职责；
- Win32/Darwin 至少共享 `TunnelConfig` 数据模型；
- `#ifdef` 不能扩散到 `src/core`；
- Linux unsupported 状态必须显式、结构化，不再静默回退造成误判；
- 所有新增平台操作可被 fake/mock 测试。

---

## Agent 4：UI / Desktop RPC / Config / Credential / Error Contract

### 目标

让 UI 成为纯交互层，整理 RPC contract、配置读写、错误模型和凭据安全。

### 负责范围

```text
src/app_api.cpp 拆分到 core_api/
src/config*.cpp/hpp
src/crypto*.cpp/hpp
src/feedback/
webui/desktop/shared/desktop-contract.ts
webui/desktop/main/*
webui/src/stores/*
```

### 主要任务

1. 拆分 `app_api.cpp` 巨型 dispatcher。
2. 调整 `vpn.connect` 请求：传 `desired_connected` / `auto_reconnect` 意图，不传 `retry_limit`。
3. 统一状态响应，显示 core phase、helper mode、last_error、reconnect info。
4. 整理 config/profile 读写，避免多处重复加载保存。
5. 凭据存储：Windows DPAPI / macOS Keychain 设计与最小实现；Linux 降级策略。
6. 禁止密码/auth_token 通过命令行参数传递；改用 stdin、secure temp pipe、OS credential handle 或 helper/core 内存通道。
7. 错误 contract 从字符串升级为结构化对象。
8. UI 中对旧字段做兼容显示。

### 不允许做

- 不改 helper 内部 lease 逻辑；
- 不改平台 route/DNS；
- 不改协议解析；
- 不绕过 core 直接调用 helper V2 特权操作。

### 交付物

- `src/core_api/app_rpc_dispatcher.hpp/.cpp`
- `src/core_api/vpn_actions.hpp/.cpp`
- `src/core_api/config_actions.hpp/.cpp`
- `src/core_api/service_actions.hpp/.cpp`
- `src/core_api/route_actions.hpp/.cpp`
- `src/feedback/error_contract.hpp/.cpp`
- `src/platform/common/credential_store.hpp`
- `src/platform/win32/win_credential_store.cpp`
- `src/platform/darwin/darwin_keychain_store.mm`
- 更新 `webui/desktop/shared/desktop-contract.ts`
- `docs/DESKTOP_RPC_V2.md`

### 验收标准

- UI 不再生成 `retry_limit`；
- `vpn.connect` 请求中不含明文密码命令行参数；
- `status.get` 能显示 `Connected/Reconnecting/Failed/CleaningUp`；
- 错误对象含 domain/code/recoverable/recommended_action；
- config 保存仍保持原子写；
- 旧配置可迁移；
- Electron 渲染进程 API 类型检查通过；
- 不增加 renderer 对 Node/Electron 直接访问。

---

## Agent 5：Integration / Tests / CI / Migration Guardrails

### 目标

确保五路并行重构不破坏现有功能，并把以前未运行的测试纳入 CI。

### 负责范围

```text
tests/
tests/support/
.github/workflows/
scripts/
docs/migration/
packaging smoke tests
```

### 主要任务

1. 建立 fake AnyConnect server + fake helper + fake platform ops 的集成测试框架。
2. 把现有 CTest 纳入 CI，至少在每个平台运行可运行子集。
3. 新增迁移测试：legacy start/stop/status 不破坏。
4. 新增 native core state machine 集成测试。
5. 新增 helper V2 contract tests。
6. 新增安全回归测试：命令行不出现 password/auth_token；helper V2 schema 不含凭据字段。
7. 新增 cleanup 测试：core crash/helper timeout 后 registry cleanup。
8. 增加 feature flag 与 fallback 策略测试。
9. 维护重构进度看板文档。

### 不允许做

- 不大规模改业务逻辑；
- 不私自修改 shared interface；
- 不把测试跳过当成功；
- 不删除 legacy 测试。

### 交付物

- `.github/workflows/build.yml` 更新，运行 CTest；
- `tests/support/fake_helper.*`
- `tests/support/fake_platform_network_ops.*`
- `tests/support/fake_core_ui_client.*`
- `tests/integration/native_core_connect_flow_test.cpp`
- `tests/integration/helper_timeout_cleanup_test.cpp`
- `tests/security/no_secret_in_argv_test.cpp`
- `docs/MIGRATION_CHECKLIST.md`
- `docs/REGRESSION_MATRIX.md`

### 验收标准

- CI 在 Windows/macOS/Linux 至少运行构建 + 单元测试；
- 新增测试能在无真实 VPN 服务器下跑；
- fake server 覆盖 auth success、auth failed、transport closed；
- helper timeout cleanup 可复现；
- 每个 Agent 的 PR 都能被同一套测试矩阵检查；
- 发布前 checklist 包含旧路径回归、native 新路径回归、oneshot/resident 两种 helper 模式。

---

## 12. 并行开发时间线建议

### Round 0：接口冻结

所有 Agent 同时阅读本文档和当前审计文档。只允许提交 docs 和 interface stub。

输出：

- `ARCHITECTURE_TARGET.md`
- `HELPER_PROTOCOL_V2.md`
- `CORE_STATE_MACHINE.md`
- `DESKTOP_RPC_V2.md`
- `REGRESSION_MATRIX.md`

### Round 1：低风险拆分

- Agent 1 提取 timing/startup failure；
- Agent 2 拆 helper session store / lease manager；
- Agent 3 建平台模型和 fake ops；
- Agent 4 拆 app_api dispatcher；
- Agent 5 接入 CTest 和 fake 框架。

默认功能不变。

### Round 2：新链路 shadow mode

- Core 状态机记录但不接管；
- Helper V2 能 hello/start session/heartbeat/cleanup；
- UI 能展示新 status 但仍兼容旧字段；
- fake integration 验证新链路。

### Round 3：native path 局部切换

- dev flag：`EXV_NATIVE_CORE_CONTROLLER=1`；
- native path 由 TunnelController 接管重连；
- helper V2 管控制面；
- legacy openconnect 不变。

### Round 4：默认切换与清理

- native 默认走新 core controller；
- `__vpn-supervisor` 仅 legacy 使用或 deprecated；
- helper worker 不再运行 native protocol；
- 删除或隔离重复代码；
- 完整 CI + E2E。

---

## 13. 最终验收标准

### 架构验收

- Native AnyConnect 协议只在 core/user 权限运行；
- helper 不 include protocol 目录；
- native 重连由 `TunnelController` 状态机控制；
- UI 不生成 retry_limit；
- helper transient/resident 共用同一协议；
- helper heartbeat 超时 cleanup 行为可测试；
- Windows route cleanup 不再 no-op；
- `vpn.cpp/helper.cpp/app_api.cpp` 显著变薄，并且新逻辑在对应目录。

### 功能验收

- 用户可连接、断开、查看状态；
- auto_reconnect on/off 行为正确；
- 断网后 on 会重连，off 不重连；
- 用户主动断开不重连；
- helper 拒绝/不可用时 UI 有明确错误；
- transient helper 超时后退出；
- resident helper 超时后清理但待命；
- Electron 和 CLI 行为一致。

### 安全验收

- 明文密码不出现在进程命令行；
- helper V2 不接收 password/cookie；
- helper 不解析 AnyConnect XML/HTML；
- helper IPC 有版本协商、调用方校验、schema 校验；
- route/DNS apply 有 policy validation；
- stale cleanup 可恢复；
- 密钥使用 OS credential store 或有明确降级告警。

### 测试验收

- CI 运行 CTest；
- fake server/fake helper/fake platform 可在无校园 VPN 下测试核心逻辑；
- 重连状态机测试覆盖 recoverable/unrecoverable/user-disconnect/auto-reconnect off；
- helper lease 测试覆盖 transient/resident；
- 平台网络操作模型有单元测试；
- 安全回归测试覆盖 secret 不进 argv。

---

## 14. 给子智能体的统一执行 Prompt 模板

每个子智能体执行时都应附加这段约束：

```text
你正在参与 ECNU-VPN 架构整改。你的工作必须遵守以下规则：

1. 只修改你被分配的目录和文件；如需改共享接口，先更新 docs/INTERFACE_CHANGELOG.md 并说明兼容策略。
2. 不要删除 legacy 路径，除非任务明确要求；默认所有改造都必须 feature-flag 或兼容旧行为。
3. 不允许把 helper 变成通用 shell 或命令执行器。
4. 不允许让 helper V2 接收 password、cookie、webvpn_session、auth_token。
5. 不允许在 helper 中新增 AnyConnect 协议解析依赖。
6. 不允许把每个 IP packet 通过 helper IPC 转发。
7. 所有新增 public interface 必须有测试或 fake 实现。
8. 所有状态机、重连、cleanup 改动必须有可重复测试。
9. 修改完成后输出：改了哪些文件、行为变化、兼容性、测试结果、剩余风险。
```

---

## 15. 结论

当前架构不是推倒重来，而是做一次“owner 归位”：

```text
UI owner = interaction
Core owner = tunnel lifecycle + protocol + reconnect + packet pump
Helper owner = privileged control-plane ops + lease cleanup
OS owner = resident helper/service process supervision
```

最重要的第一步不是写平台细节，而是冻结共享接口：`TunnelController`、`ReconnectPolicy`、`HelperProtocolV2`、`PlatformNetworkOps`、`DesktopRpcV2`。这些接口稳定后，5 个智能体才能安全并行。
