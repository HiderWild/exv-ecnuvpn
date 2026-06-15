# EXV 重构任务

> 来源：`docs/architecture/new_start_point.md`。
> 本文是任务层拆解。任务按阶段组织，并保留 epic 编号，便于分配给不同 agent / lane。

## 阶段总览

| 阶段 | 范围 | 主要任务 |
|------|------|----------|
| Phase 0 | 规约冻结 | A1-A3, B1-B2 |
| Phase 1 | Core / Helper 边界 | C1-C2, D1 |
| Phase 2 | IPC / Resolver / 状态 | G1-G2, RPC / capabilities 集成 |
| Phase 3 | Service 模式统一 | D2-D3, H2 service 命令 |
| Phase 4 | One-shot 模式补齐 | E1-E3, F1-F2 |
| Phase 5 | CLI / Desktop 控制端重构 | H1-H2, I1-I4 |
| Phase 6 | 验收与发布门禁 | J1-J4, merge blocker 清理 |

## Phase 0：规约冻结

### A1. 冻结产品运行模式决策

目标：

```text
EXV 不强制安装系统服务。
service 是推荐项，不是使用前提。
oneshot 是正式运行模式，不是临时 hack。
```

实施：

- 将 `docs/architecture/new_start_point.md` 设为本轮重构最高指示。
- 在架构文档中明确 service / oneshot / foreground 三种 helper 模式。
- 明确 Windows one-shot 是产品目标下的 merge blocker。

验收：

- 文档写明服务不是强制项。
- 文档写明 Windows oneshot 必须补齐，或必须做显式降级。

### A2. 定义组件边界文档

目标：

```text
exv-core = 核心能力库
exv-helper = 唯一特权执行体
CLI / Electron = 平级控制端
```

实施：

- 固化组件关系图。
- 标注 CLI、Electron main、renderer、helper、core 的职责。
- 列出禁止依赖方向。

验收：

- 文档中明确禁止 Electron/CLI 直接执行核心 VPN 逻辑。
- 文档中明确 renderer 禁止特权操作。

### A3. 定义 RPC 协议草案

目标：

```text
CLI/UI 通过同一套本地 RPC 控制 helper。
```

实施：

- 定义 transport。
- 定义最小 RPC 方法集合。
- 定义 `hello` / `status` / `capabilities`。
- 定义 auth token 和 endpoint 安全要求。

验收：

- RPC 草案覆盖 service 和 oneshot。
- helper 不接受任意 shell 命令。

### B1. 建立平台差异清单

目标：

```text
明确 Windows/macOS 当前 service、oneshot、fallback、helper binary 差异。
```

实施：

- 盘点 Windows 当前流程：service install、named pipe、connect、缺失 oneshot。
- 盘点 macOS 当前流程：LaunchDaemon、Unix socket、direct fallback。
- 标记所有 direct core connect 和 fallback 入口。

验收：

- 差异清单能回答：哪些能力真实存在，哪些只是 UI 假入口。

### B2. 建立平台能力矩阵

目标：

```text
让 UI/CLI 通过能力矩阵判断功能，而不是猜平台行为。
```

实施：

- 建立当前矩阵。
- 建立目标矩阵。
- 将矩阵字段映射到 helper `hello` / resolver capabilities。

目标最终状态：

```json
{
  "windows": {
    "service_mode": true,
    "oneshot_mode": true,
    "direct_fallback": false,
    "helper_binary": true
  },
  "darwin": {
    "service_mode": true,
    "oneshot_mode": true,
    "direct_fallback": false,
    "helper_binary": true
  }
}
```

验收：

- UI 不再硬编码“尝试 fallback”。
- Windows 未实现 oneshot 前，UI 不展示临时连接。

## Phase 1：Core / Helper 边界

### C1. 抽离核心 VPN 能力库

目标：

```text
把 vpn start/stop/status/cleanup 从 CLI/UI/helper 代码中抽离到 exv-core。
```

实施：

- 建立 core 层接口：

```text
core::vpn::connect(request)
core::vpn::disconnect()
core::vpn::status()
core::vpn::cleanup()
core::route::apply()
core::route::restore()
core::dns::apply()
core::dns::restore()
core::adapter::create()
core::adapter::remove()
```

验收：

- CLI 不再直接散落调用平台网络逻辑。
- helper 可以调用同一套 core。
- macOS 和 Windows 差异被封装到 platform adapter。

依赖：

- A2

### C2. 定义平台适配接口

目标：

```text
把 Windows/macOS 差异封进 platform adapter。
```

实施：

- 定义接口：

```text
IAdapterManager
IRouteManager
IDnsManager
IVpnProcessRunner
IPrivilegeContext
IServiceManager
IIpcTransport
```

验收：

- core 调用抽象接口。
- platform/windows 实现 Windows 行为。
- platform/darwin 实现 macOS 行为。
- UI/CLI 中不散落平台网络逻辑。

依赖：

- C1

### D1. 建立 exv-helper 统一入口

目标：

```text
让 exv-helper 成为跨平台统一特权执行体。
```

实施：

- 支持命令：

```bash
exv-helper --service
exv-helper --oneshot --endpoint <endpoint> --owner <uid-or-sid> --parent-pid <pid>
```

- 将 `helper::daemon_main()` 收敛为共同入口。
- 为 service / oneshot / foreground 准备统一启动参数解析。

验收：

- Windows `exv-helper.exe --service` 可用。
- macOS 新增 `exv-helper` 可执行文件。
- oneshot 和 service 共用 RPC handler。

依赖：

- A2
- A3
- C1

## Phase 2：IPC / Resolver / 状态

### G1. 实现 shared BackendResolver

目标：

```text
CLI 和 Electron main 使用同一套后端选择逻辑。
```

实施：

- 实现输入：

```json
{
  "preferred_mode": "auto | service | oneshot",
  "allow_oneshot": true,
  "allow_service_start": true
}
```

- 实现输出：

```json
{
  "backend": "service | oneshot",
  "endpoint": "...",
  "transport": "...",
  "auth_token": "...",
  "pid": 1234
}
```

- 自动流程：
  1. 检查 service helper 是否运行。
  2. 检查 service 是否已安装，必要时尝试启动。
  3. 允许 oneshot 时触发平台提权并启动 helper。
  4. 否则返回结构化 unavailable 错误。

验收：

- CLI 和 Electron main 不各自实现一套平台判断。
- Windows/macOS 都通过 BackendResolver 决定连接谁。

依赖：

- A3
- D1

### G2. 统一 helper_unavailable 错误语义

目标：

```text
消除 helper_unavailable 后 UI 盲目尝试 fallback 的行为。
```

实施：

- 引入错误分类：

```text
service_not_installed
service_installed_not_running
service_start_failed
oneshot_not_supported
oneshot_elevation_denied
helper_rpc_failed
auth_failed
vpn_start_failed
```

验收：

- UI 可以根据错误显示准确提示。
- Windows 未实现 oneshot 时，不会误导用户。

依赖：

- G1

### R2-IPC. 实现 helper hello/status/capabilities

目标：

```text
让 UI/CLI 以 helper 返回能力作为行为来源。
```

实施：

- 在 helper RPC 中实现 `hello`。
- 在 helper RPC 中实现结构化 `status`。
- 返回 mode、platform、transport、capabilities。

验收：

- UI 不依赖平台字符串决定按钮。
- CLI 和 UI 使用同一状态模型。

依赖：

- A3
- D1

## Phase 3：Service 模式统一

### D2. macOS 统一到 exv-helper --service

目标：

```text
macOS launchd 不再启动 exv 主程序内部隐藏模式。
```

实施：

- 修改 service install：

```text
  /usr/local/bin/exv-helper --service
```

- 调整稳定二进制复制路径。
- 确认 service 启动后仍暴露 `/var/run/exv-helper.sock`。

验收：

- 新安装的 plist 指向 `exv-helper --service`。
- 旧 plist 可被卸载重装或迁移。

依赖：

- D1

### D3. 清理旧 daemon 入口

目标：

```text
旧入口不能继续进入生产 helper daemon。
```

实施：

- 旧入口返回结构化错误或迁移提示。
- service install 和 launchd plist 只写 `exv-helper --service`。
- 测试禁止旧入口被当作合法 helper 启动。

验收：

- 旧命令不会启动生产 helper daemon。
- 新安装不再生成旧 plist。

依赖：

- D2

### H2. CLI service 命令重构

目标：

```text
service install/uninstall/start/stop 只管理系统服务，不执行连接。
```

实施：

- `exv service install`：
  - Windows 注册 `exv-helper.exe --service`。
  - macOS 注册 `exv-helper --service`。
- `exv service uninstall`：
  - 删除系统服务。
  - 不删除 CLI 本体。
- `exv service start/stop`：
  - 仅操作系统服务管理器。

验收：

- 服务安装与应用安装分离。
- service 未安装时 CLI 仍可运行。

依赖：

- D1
- D2

## Phase 4：One-shot 模式补齐

### E1. 设计 Windows one-shot bootstrap

目标：

```text
实现 Windows 未安装 service 时，本次 UAC 提权连接。
```

实施：

- Electron main / CLI 生成：
  - `session_id`
  - `pipe_path`
- 通过 UAC 启动：

```bash
exv-helper.exe --oneshot --pipe <pipe> --owner <sid> --parent-pid <pid>
```

注意：

- 不依赖 elevated 进程 stdout 返回 pipe 信息。
- endpoint 由普通用户进程预先生成，合法性由随机 endpoint、OS peer、owner 和 parent pid 校验。

验收：

- 服务未安装时可以启动 elevated `exv-helper`。
- UI/CLI 能连接它创建的 named pipe。

依赖：

- D1
- A3

### E2. 实现 Windows oneshot named pipe server

目标：

```text
exv-helper --oneshot 创建临时 named pipe 并提供 RPC。
```

实施：

- pipe name：

```text
\\.\pipe\exv-oneshot-{session_id}
```

- 启动后：
  - create pipe
  - wait client
  - validate OS peer and require first Hello
  - serve RPC
  - after disconnect/shutdown cleanup and exit

验收：

- UI connect。
- 弹 UAC。
- helper 启动。
- pipe 建立。
- auth 成功。
- `vpn.connect` 成功。

依赖：

- E1

### E3. 移除 Windows no direct fallback 策略

目标：

```text
不再写死 Windows always requires helper service。
```

实施：

- 不改成直接调用 core。
- 改成通过 BackendResolver 启动 / 连接 oneshot helper。
- 将 capability 中 Windows `oneshot_mode` 改为真实实现状态。

验收：

- 原 native policy 不再返回空 fallback。
- UI 不再调用未完成能力。

依赖：

- E2

### F1. 把 macOS direct fallback 改为 exv-helper --oneshot

目标：

```text
废弃 exv desktop-rpc 直接 vpn::start_with_password 的临时提权路径。
```

实施：

```text
旧：
osascript -> exv desktop-rpc vpn.connect allow_direct_fallback -> vpn::start_with_password

新：
osascript -> exv-helper --oneshot --socket <path> --owner <uid> --parent-pid <pid>
UI/CLI -> socket RPC -> vpn.connect
```

验收：

- macOS 服务未安装时仍可临时连接。
- 执行体是 `exv-helper --oneshot`。
- 不再由 `exv desktop-rpc` 直接执行 core。

依赖：

- D1
- A3

### F2. macOS 临时 socket 权限与清理

目标：

```text
保证 root 启动的 oneshot helper 创建的 socket 可被当前用户连接且安全。
```

实施：

- socket path：

```text
/tmp/exv-{uid}-{session_id}.sock
```

- 创建后：
  - chown 到目标用户。
  - chmod 0600 或等价控制。
  - 验证 auth token。
  - 退出时删除 socket。

验收：

- 普通用户 UI 能连接 socket。
- 其他用户不能随意连接。
- helper 退出后 socket 文件不存在。

依赖：

- F1

## Phase 5：CLI / Desktop 控制端重构

### H1. CLI connect 改为 helper client

目标：

```text
CLI 不直接执行 VPN 核心逻辑。
```

实施：

```text
exv connect:
  BackendResolver(auto)
  RPC vpn.connect

exv connect --temporary:
  BackendResolver(oneshot)
  RPC vpn.connect

exv disconnect:
  connect existing helper
  RPC vpn.disconnect
```

验收：

- service 模式和临时模式下 CLI 行为一致。
- CLI 与 UI 看到同一 helper 状态。

依赖：

- G1
- D1

### I1. UI 状态模型重构

目标：

```text
UI 明确区分 App/UI、Service、Helper、VPN 四种状态。
```

实施：

- 修改 store / desktop contract / 页面逻辑，使其独立表达：
  - app open
  - service installed
  - helper running
  - vpn connected
  - backend mode
  - last error

验收：

- UI 不再把“打开 UI”和“启动连接”绑定。
- UI 不再把“安装服务”和“安装应用”绑定。

依赖：

- A1
- G2

### I2. UI capability-driven 行为

目标：

```text
UI 不再硬编码平台判断。
```

实施：

- 读取 capabilities：
  - `service_mode`
  - `oneshot_mode`
  - `service_installed`
  - `helper_running`
  - `temporary_connect`
- 根据能力展示按钮和提示。

验收：

- Windows oneshot 未完成时，不显示“本次提权连接”。
- Windows oneshot 完成后，自动显示。
- macOS helper 迁移前后 UI 不需要大改。

依赖：

- B2
- G1

### I3. connectElevated 语义重命名

目标：

```text
把容易误解为直接提权执行连接的 vpn.connectElevated 改为 oneshot 语义。
```

实施：

- 推荐重命名为：

```text
vpn.connectTemporary
```

或：

```text
vpn.connectOneshot
```

- 语义定义为：启动 / 连接 one-shot elevated helper，然后通过 RPC 发起连接。
- `allow_direct_fallback` 如保留，只能作为 deprecated compatibility。

验收：

- 目标路径中不再出现 `allow_direct_fallback`。
- UI / backend 对临时连接使用一致命名。

依赖：

- G1
- F1
- E2

### I4. 连接进度状态拆分与日志化

目标：

```text
把“等待授权”拆分为授权、临时 helper 就绪、创建虚拟网卡、连接 VPN、写入路由、网络就绪等阶段。
```

实施：

- 在 helper / app API / desktop contract 中增加连接阶段事件或状态字段。
- one-shot 授权成功并拿到临时 helper 后，UI 立即退出“等待授权”，显示后续实际阶段。
- 将关键阶段写入日志，便于调试卡点：
  - requesting authorization
  - oneshot helper ready
  - starting openconnect
  - creating tunnel adapter
  - configuring interface
  - writing routes
  - network ready
- 阶段可能很快闪过，但仍允许 UI 展示，以提升透明度。
- 虚拟网卡探测提示仅在连接后展示；未连接时即使探测到上游虚拟网卡，也不在主界面提示，避免造成“未连接就改网卡”的误解。

验收：

- UAC / 管理员授权完成后，UI 不再继续显示“等待授权”。
- 连接期间的主要阶段可在 UI 和日志中对应起来。
- 连接失败时错误能指向最后一个阶段。
- 未连接状态不展示上游虚拟网卡提示。

依赖：

- I1
- G2

## Phase 6：验收与发布门禁

### J1. Windows service 模式测试

脚本：

```text
1. 卸载旧服务
2. 安装 service
3. 启动 service
4. UI connect
5. CLI status
6. UI disconnect
7. 停止 service
8. 卸载 service
```

通过标准：

- 无 UAC 重复弹窗。
- named pipe 可连接。
- helper 状态正确。
- 路由 / DNS 清理正确。

### J2. Windows one-shot 模式测试

脚本：

```text
1. 确认 service 未安装
2. 打开 UI
3. 点击本次临时连接
4. UAC 弹窗
5. 确认后连接成功
6. 检查没有新增 Windows Service
7. 断开连接
8. 检查 exv-helper oneshot 退出
9. 检查路由/DNS/虚拟网卡清理
```

通过标准：

- 不安装服务也能连接。
- 失败时错误提示准确。
- 取消 UAC 时 UI 显示 `oneshot_elevation_denied`。

### J3. macOS service 模式测试

脚本：

```text
1. 卸载旧 LaunchDaemon
2. 安装新 service
3. 检查 plist ProgramArguments
4. 确认为 exv-helper --service
5. launchctl bootstrap
6. UI connect
7. CLI status
8. disconnect
9. uninstall
```

通过标准：

- 不再依赖旧 helper daemon 入口。
- socket 正常创建。
- 权限正确。

### J4. macOS one-shot 模式测试

脚本：

```text
1. 确保 LaunchDaemon 未安装
2. UI 点击本次临时连接
3. osascript 管理员授权
4. exv-helper --oneshot 启动
5. socket 创建
6. UI RPC 连接
7. VPN connect
8. disconnect
9. helper 退出
10. socket 删除
```

通过标准：

- 无 service 安装。
- 临时连接成功。
- 清理完整。

## 任务依赖图

```text
A1 架构决策
  ↓
A2 组件边界 ──→ C1 core 抽离 ──→ D1 helper 统一入口
  ↓                  ↓                  ↓
A3 RPC 协议 ──────→ C2 平台适配 ─────→ E/F oneshot 实现
  ↓                                     ↓
B2 capability 矩阵 ─────────────────→ G BackendResolver
                                          ↓
                         H CLI 重构 ─────┼───── I UI 重构
                                          ↓
                                      J 测试验收
```

## 并行策略

可并行分配：

- Agent 1：A1 / A2 / A3 文档和协议冻结。
- Agent 2：C1 / C2 core 抽离。
- Agent 3：Windows service / oneshot helper。
- Agent 4：macOS `exv-helper` 迁移 / oneshot helper。
- Agent 5：Electron UI 状态模型 / capability UI。
- Agent 6：CLI command 重构。
- Agent 7：测试脚本 / 验收用例 / CI。

约束：

- UI agent 不得自定义后端协议。
- CLI agent 不得自定义另一套 resolver。
- Windows/macOS agent 不得擅自修改 core 公共 API，除非架构负责人批准。

## 冲突高发目录

需要协调锁定：

```text
core/**
helper/**
app_api/**
desktop-rpc/**
electron/main/**
platform/windows/**
platform/darwin/**
service/**
```

建议分配：

```text
core/**:
  Core agent 独占

platform/windows/**:
  Windows agent 独占

platform/darwin/**:
  macOS agent 独占

electron/renderer/**:
  UI agent 独占

electron/main/backend/**:
  UI agent + BackendResolver agent 协调

cli/**:
  CLI agent 独占

docs/architecture/**:
  架构负责人维护
```

## Merge Blocker 清单

1. Windows one-shot elevated helper：产品目标要求不强制安装服务，因此推荐作为必须实现项。
2. UI 不得隐式尝试不存在的 fallback。
3. macOS LaunchDaemon 入口必须迁移到 `exv-helper --service`，或至少明确短期兼容和长期迁移。
4. CLI 必须独立于 service 安装。
5. UI / CLI / helper 状态模型必须统一。
