# EXV 重构需求

> 来源：`docs/architecture/new_start_point.md`。
> 本文是需求层拆解。若与旧文档冲突，以 `docs/architecture/new_start_point.md` 和 `00-constitution.md` 为准。

## 阶段划分

| 阶段 | 名称 | 目标 |
|------|------|------|
| Phase 0 | 规约冻结 | 固化最高原则、组件边界、RPC 草案和能力矩阵 |
| Phase 1 | Core / Helper 边界 | 抽离 `exv-core`，统一 `exv-helper` 入口 |
| Phase 2 | IPC / Resolver / 状态 | 定义 RPC、capabilities、BackendResolver 和错误语义 |
| Phase 3 | Service 模式统一 | Windows/macOS service 均启动 `exv-helper --service` |
| Phase 4 | One-shot 模式补齐 | Windows/macOS 均支持 `exv-helper --oneshot` |
| Phase 5 | CLI / Desktop 控制端重构 | CLI 和 Electron 都通过 resolver + helper RPC 工作 |
| Phase 6 | 验收与发布门禁 | 完成 service、oneshot、UI/CLI 一致性和能力真实性验证 |

## Phase 0：规约冻结

### R0.1 最高指示

`docs/architecture/new_start_point.md` 必须被视为新的最高指示。仓库中旧路线图或旧计划如有冲突，均以该文档为准。

验收：

- 本三层文档存在并声明裁决规则。
- 后续重构任务引用本三层文档，而不是旧的冲突路线图。

### R0.2 组件边界

必须冻结以下边界：

```text
Vue Renderer -> Electron IPC -> Electron Main / Desktop Backend
  -> BackendResolver -> exv-helper RPC -> exv-core

exv CLI -> BackendResolver / service manager / helper RPC -> exv-helper
```

验收：

- Renderer 不直接执行特权逻辑。
- CLI 不直接执行 VPN 核心逻辑。
- Electron 不直接操作网卡、路由、DNS 或 OpenConnect。

### R0.3 产品运行模式

必须明确支持两类正式使用模式：

- 临时模式：不安装服务，本次管理员授权，启动 one-shot helper。
- 后台服务模式：安装 helper service，长期后台运行。

验收：

- 服务不是使用前提。
- 如果某平台尚未支持临时模式，UI 必须如实隐藏或降级提示。

## Phase 1：Core / Helper 边界

### R1.1 exv-core 职责

`exv-core` 只负责核心能力：

- VPN connect / disconnect / status / cleanup。
- 路由 apply / restore。
- DNS apply / restore。
- 虚拟网卡 create / remove。
- OpenConnect 启动、监控和退出。
- 重连和状态机。

验收：

- CLI、Electron、helper 不再散落平台网络逻辑。
- 平台差异通过 platform adapter 注入或封装。

### R1.2 exv-helper 职责

`exv-helper` 是唯一特权执行体，必须支持：

```bash
exv-helper --service
exv-helper --oneshot --endpoint <endpoint> --owner <uid-or-sid> --parent-pid <pid>
```

验收：

- Windows 保持 `exv-helper.exe --service` 可用。
- macOS 新增正式 `exv-helper` 可执行文件。
- `helper::daemon_main()` 成为跨平台共同入口。

### R1.3 平台适配接口

需要将 Windows/macOS 差异封进平台适配层。建议接口包括：

- `IAdapterManager`
- `IRouteManager`
- `IDnsManager`
- `IVpnProcessRunner`
- `IPrivilegeContext`
- `IServiceManager`
- `IIpcTransport`

验收：

- 共享逻辑不新增大量平台分支。
- Windows/macOS 实现位于平台目录或平台 adapter 中。

## Phase 2：IPC / Resolver / 状态

### R2.1 Transport

平台默认 transport：

```text
Windows service:
  \\.\pipe\exv-helper

Windows oneshot:
  \\.\pipe\exv-oneshot-{session_id}

macOS service:
  /var/run/exv-helper.sock

macOS oneshot:
  /tmp/exv-{uid}-{session_id}.sock
```

验收：

- service endpoint 稳定。
- oneshot endpoint 随机化并只用于当前会话。

### R2.2 RPC 方法

helper runtime API 至少包含：

```text
hello
status
vpn.connect
vpn.disconnect
vpn.reconnect
vpn.getState
logs.tail
events.subscribe
config.get
config.set
helper.shutdown
```

服务管理不放入 helper runtime API；服务未安装时 helper 可能不存在。服务管理由 CLI / Electron main 通过平台 service manager 执行：

```text
service.status
service.install
service.uninstall
service.start
service.stop
```

验收：

- helper 不接受任意 shell 命令。
- helper 只接受结构化 RPC action。

### R2.3 hello / capabilities

helper 必须提供 `hello`，并返回 mode、transport、platform 和 capabilities。

示例：

```json
{
  "name": "exv-helper",
  "version": "x.y.z",
  "platform": "windows",
  "mode": "service",
  "transport": "named-pipe",
  "capabilities": {
    "vpn_connect": true,
    "vpn_disconnect": true,
    "logs": true,
    "events": true,
    "temporary_connect": true,
    "service_mode": true
  }
}
```

验收：

- UI 不根据平台字符串猜能力。
- CLI 和 UI 使用同一能力语义。

### R2.4 BackendResolver

必须实现 shared BackendResolver。

输入：

```json
{
  "preferred_mode": "auto | service | oneshot",
  "allow_oneshot": true,
  "allow_service_start": true
}
```

输出：

```json
{
  "backend": "service | oneshot",
  "endpoint": "...",
  "transport": "...",
  "auth_token": "...",
  "pid": 1234
}
```

验收：

- CLI 和 Electron main 不各自实现一套平台判断。
- Windows/macOS 都通过 BackendResolver 决定连接 service 还是 oneshot。

### R2.5 结构化错误语义

必须替代含混的 `helper_unavailable` 后盲目 fallback 行为。

错误分类：

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
- Windows oneshot 未完成时，不会误导用户尝试临时连接。

### R2.6 安全要求

必须满足：

1. oneshot endpoint 必须带随机 `session_id`。
2. oneshot 必须带 `auth_token`。
3. helper 收到第一条请求必须验证 `auth_token`。
4. service pipe/socket 必须有合理 ACL / 文件权限。
5. helper 不信任 renderer 输入。
6. 所有来自 UI/CLI 的配置都必须校验。

验收：

- 取消授权、auth 失败、RPC 失败均有结构化错误。
- oneshot helper 退出后清理 endpoint。

## Phase 3：Service 模式统一

### R3.1 Windows service

目标流程：

```text
service.install -> UAC RunAs -> exv service install
  -> SCM CreateService
  -> binary_path = exv-helper.exe --service
  -> SCM StartService
  -> exv-helper.exe --service
  -> named pipe \\.\pipe\exv-helper
```

验收：

- 服务安装后，UI 连接不再弹 UAC。
- UI 关闭后连接可持续。
- 服务崩溃后 SCM 可重新拉起。
- CLI `exv status` 能连接同一个 helper。

### R3.2 macOS service

目标流程：

```text
service.install -> administrator authorization
  -> copy stable exv-helper to /usr/local/bin/exv-helper
  -> write LaunchDaemon plist
  -> ProgramArguments = /usr/local/bin/exv-helper --service
  -> launchctl bootstrap system
  -> /var/run/exv-helper.sock
```

验收：

- plist 不再启动旧的隐藏 helper daemon 入口。
- UI / CLI 均可连接 `/var/run/exv-helper.sock`。
- 旧 plist 可迁移或重装。

### R3.3 旧 daemon 入口清理

旧的隐藏 helper daemon 入口不再是合法生产入口。

验收：

- 旧命令不会进入 helper daemon。
- 新安装不再生成旧 plist。

## Phase 4：One-shot 模式补齐

### R4.1 Windows one-shot

Windows 必须实现：

```bash
exv-helper.exe --oneshot --pipe <pipe> --owner <sid> --parent-pid <pid>
```

目标流程：

```text
BackendResolver detects no service
  -> Electron main / CLI generates session_id and named_pipe_path
  -> PowerShell Start-Process -Verb RunAs exv-helper.exe --oneshot ...
  -> helper creates named pipe and verifies peer owner
  -> UI/CLI connects pipe and sends Hello first
  -> vpn.connect
  -> disconnect cleanup
  -> helper exits
```

验收：

- 完全卸载 service 后，UI 点击临时连接会弹 UAC 并成功连接。
- 系统中没有新注册 Windows Service。
- 断开后 elevated helper 退出。
- 路由、DNS、虚拟网卡被清理。

### R4.2 macOS one-shot

macOS 必须迁移为：

```bash
exv-helper --oneshot --socket <path> --owner <uid> --parent-pid <pid>
```

目标流程：

```text
BackendResolver detects no service
  -> Electron main / CLI generates socket_path, auth_token
  -> osascript administrator authorization starts exv-helper --oneshot
  -> helper creates socket and sets permissions
  -> UI/CLI connects socket and sends auth token
  -> vpn.connect
  -> disconnect cleanup
  -> helper exits
```

验收：

- 不安装 LaunchDaemon 时仍可临时连接。
- 执行体是 `exv-helper --oneshot`，不是 `exv desktop-rpc` 直接执行 core。
- helper 退出后 socket 删除。

### R4.3 取消 direct fallback 目标路径

目标架构中不再保留 `allow_direct_fallback` 作为正常路径。

验收：

- Windows 不再写死 no direct fallback。
- macOS direct fallback 只作为 deprecated compatibility。
- `vpn.connectElevated` 语义改为 `vpn.connectTemporary` 或 `vpn.connectOneshot`。

## Phase 5：CLI / Desktop 控制端重构

### R5.1 CLI

CLI 不再是核心执行者，而是：

- 用户入口。
- service manager client。
- helper bootstrapper。
- RPC client。

必须支持：

```bash
exv status
exv connect
exv disconnect
exv connect --temporary
exv service install
exv service uninstall
exv service start
exv service stop
exv helper --foreground
```

验收：

- `exv connect` 优先连接 service helper，必要时启动已安装服务，最后按策略启动 oneshot。
- `exv connect --temporary` 明确走 oneshot，不安装系统服务。
- `exv service install` 只注册 helper service，不代表安装 CLI。

### R5.2 Electron Desktop

Electron Desktop 是用户主入口，但不是特权执行体。

Renderer 只发 UI 意图，Electron main / Desktop Backend 负责：

- service manager 操作。
- BackendResolver。
- helper RPC client。
- 平台提权 bootstrap。

验收：

- Renderer 无 PowerShell / osascript / child_process 特权路径。
- Electron 不直接操作网卡、路由、DNS 或 OpenConnect。

### R5.3 UI 状态模型

UI 必须明确区分：

- App/UI 是否打开。
- Service 是否安装。
- Helper 是否运行。
- VPN 是否连接。

验收：

- UI 不再把“打开 UI”和“启动连接”绑定。
- UI 不再把“安装服务”和“安装应用”绑定。

### R5.4 UI capability-driven 行为

UI 读取 capabilities 决定展示：

- 服务未安装：根据 `temporary_connect` 展示临时连接，根据 service capability 展示安装后台服务。
- 服务已安装但未运行：展示启动服务和可用的临时连接。
- 服务运行中：展示连接、断开、日志等 helper 能力。

验收：

- Windows oneshot 未完成时，不显示临时连接。
- Windows oneshot 完成后，capability 改为 true 即自动显示。
- macOS helper 迁移前后 UI 不需要大改。

## Phase 6：验收与发布门禁

### R6.1 Windows service 验收

必须验证：

- 卸载旧服务。
- 安装 service。
- 启动 service。
- UI connect。
- CLI status。
- UI disconnect。
- 停止 service。
- 卸载 service。

通过标准：

- 无 UAC 重复弹窗。
- named pipe 可连接。
- helper 状态正确。
- 路由 / DNS 清理正确。

### R6.2 Windows one-shot 验收

必须验证：

- service 未安装。
- UI 临时连接。
- UAC 确认后连接成功。
- 没有新增 Windows Service。
- disconnect 后 oneshot helper 退出。
- UAC 取消时 UI 显示 `oneshot_elevation_denied`。

### R6.3 macOS service 验收

必须验证：

- 卸载旧 LaunchDaemon。
- 安装新 service。
- plist ProgramArguments 为 `exv-helper --service`。
- UI connect。
- CLI status。
- disconnect。
- uninstall。

### R6.4 macOS one-shot 验收

必须验证：

- LaunchDaemon 未安装。
- UI 临时连接。
- 管理员授权。
- `exv-helper --oneshot` 启动。
- socket 创建。
- UI RPC 连接。
- VPN connect。
- disconnect 后 helper 退出，socket 删除。

### R6.5 UI / CLI 一致性验收

必须验证：

- UI 连接后，`exv status` 能看到连接状态。
- CLI disconnect 后，UI 能看到断开状态。
- UI 不展示平台未支持的假能力。
