# EXV 重构宪法

> 来源：`docs/architecture/new_start_point.md`。
> 本文是该文档的宪法层拆解，用于裁决架构方向、权限边界和产品原则。

## 0. 效力与裁决规则

1. `docs/architecture/new_start_point.md` 是本轮客户端拆解和 helper 重构的最高指示。
2. 本目录下三层文档是对 `docs/architecture/new_start_point.md` 的结构化拆解：
   - `00-constitution.md`：不可违背的宪法原则。
   - `10-requirements.md`：按阶段冻结的需求。
   - `20-tasks.md`：按阶段执行的任务。
3. 仓库中任何旧路线图、旧设计、旧计划或历史实现与 `docs/architecture/new_start_point.md` 冲突时，以 `docs/architecture/new_start_point.md` 和本三层拆解为准。
4. 旧文档可作为历史背景，但不能覆盖本宪法的架构边界。
5. 如果实现中发现本三层文档与 `docs/architecture/new_start_point.md` 表达不一致，以该文档原文为最终裁决来源，并同步修订本三层文档。

## 1. 核心架构

EXV 的目标架构是：

```text
exv-core
  ↓ called by
exv-helper
  ↓ local IPC / RPC
CLI / Electron Desktop / other clients
```

其中：

- `exv-core` 只负责 VPN、虚拟网卡、路由、DNS、OpenConnect、清理、重连、状态机等核心能力。
- `exv-helper` 是唯一正式的特权执行体。
- CLI 是普通用户控制端、service manager client、helper bootstrapper、RPC client。
- Electron Desktop 是普通用户 UI，renderer 只表达 UI 意图，Electron main / Desktop Backend 负责连接 helper。
- 服务不是核心逻辑本身，服务只是 `exv-helper` 的一种运行方式。

核心一句话：

> EXV 的核心不是 CLI，也不是 Electron，而是可被 service 或 one-shot 托管的 privileged helper backend。CLI 和 UI 只是两个平级控制端。

## 2. 不可违背原则

### 2.1 Helper 是唯一特权执行体

所有高权限网络操作必须归属于 `exv-helper` 和 `exv-core`：

- 创建或删除虚拟网卡。
- 修改路由。
- 写系统 DNS。
- 启动和管理 OpenConnect。
- 清理路由、DNS、网卡和连接状态。

CLI 和 Electron 不得作为长期的核心执行体。

### 2.2 CLI 与 Electron 是平级控制端

CLI 和 Electron 都只能通过本地 RPC / IPC 控制 helper。二者不得各自维护互不一致的连接状态、fallback 逻辑或平台判断。

### 2.3 安装应用不等于安装服务

安装 EXV 应用或解压程序后，磁盘上应存在：

- `exv`
- `exv-helper`
- Electron desktop app
- 必要资源文件

但系统服务可以尚未安装。服务安装只表示“把已经存在的 `exv-helper` 注册到系统服务管理器”。

### 2.4 不强制安装系统服务

服务是推荐项，不是使用前提。用户必须可以选择：

- 临时模式：每次连接时进行一次管理员授权，不安装系统服务。
- 后台服务模式：安装 helper service，获得后台运行、自动重连、崩溃恢复、开机自启和免重复授权。

如果某平台尚未实现临时模式，UI 必须真实反映能力缺失，不得展示假按钮或隐式尝试不存在的 fallback。

### 2.5 Service 不得启动 UI

禁止：

```text
service / daemon -> Electron UI
```

正确关系是：

```text
user opens Electron -> Electron connects helper
```

服务是系统后台进程，UI 是用户会话进程。两者必须保持边界。

### 2.6 Renderer 不得执行特权逻辑

Vue renderer 只允许发起 UI 意图，例如：

- `connect`
- `disconnect`
- `installService`
- `uninstallService`
- `startService`
- `stopService`
- `getStatus`
- `getLogs`

Renderer 禁止直接：

- 执行 PowerShell / osascript。
- 调用 `exv-helper`。
- 操作路由、网卡或 DNS。
- 读写特权配置。

### 2.7 Capability-driven 是 UI 行为准则

UI 不得根据平台字符串猜测能力。所有可见动作都必须来自 capabilities，例如：

- `service_mode`
- `oneshot_mode`
- `temporary_connect`
- `service_installed`
- `helper_running`
- `vpn_connect`
- `vpn_disconnect`
- `logs`
- `events`

如果能力为 false 或未知，UI 不得展示可执行入口。

### 2.8 BackendResolver 必须统一

CLI 和 Electron main 必须使用同一套 BackendResolver 逻辑：

```text
1. 优先连接已运行 service helper
2. 服务已安装但未运行时，尝试启动 service
3. 允许 one-shot 时，触发平台提权并启动 exv-helper --oneshot
4. 否则返回结构化 helper_unavailable 错误
```

任何客户端不得自定义另一套 resolver。

## 3. Helper 运行模式

`exv-helper` 必须支持三种模式：

```bash
exv-helper --service
exv-helper --oneshot --endpoint <pipe-or-socket> --owner <uid-or-sid> --parent-pid <pid>
```

### 3.1 Service 模式

长期后台服务：

- Windows：Windows Service / SCM，IPC 为 Named Pipe。
- macOS：launchd / LaunchDaemon，IPC 为 Unix Domain Socket。

Service 模式用于开机自启、断线重连、崩溃恢复、UI 关闭后连接持续，以及免重复授权。

### 3.2 One-shot elevated 模式

不安装系统服务，本次连接临时提权：

- Windows：UAC 启动 `exv-helper.exe --oneshot`，使用临时 Named Pipe。
- macOS：管理员授权启动 `exv-helper --oneshot`，使用临时 Unix Socket。

One-shot helper 在连接期间存在，断开后清理路由、DNS、虚拟网卡并退出。

### 3.3 非法启动

缺少 `--service` 或完整 `--oneshot` 参数的 helper 进程必须立即退出，不得进入生产 daemon。

## 4. 平台决策

### 4.1 Windows

Windows 必须补齐真正的 `exv-helper.exe --oneshot`。在目标产品原则下，Windows 不安装服务也能本次提权连接是 merge blocker。

禁止继续把以下逻辑作为目标架构：

```cpp
// Windows always requires the helper service; no direct fallback.
return nlohmann::json{};
```

如果阶段性尚未实现 Windows one-shot，则 UI 必须隐藏临时连接能力，并明确提示需要安装 helper service。

### 4.2 macOS

macOS 当前 direct fallback 可作为短期兼容，但目标必须迁移为：

```text
/usr/local/bin/exv-helper --service
/usr/local/bin/exv-helper --oneshot
```

需要逐步废弃：

```text
exv desktop-rpc vpn.connect {"allow_direct_fallback": true}
```

## 5. 禁止架构

以下结构不得作为长期实现保留：

```text
Electron UI
  ↓ child_process
exv desktop-rpc vpn.connect
  ↓
直接调用 vpn::start_with_password()
```

`desktop-rpc` 可短期保留为兼容入口，但必须退化为 helper RPC client / backend bootstrapper。

禁止 Electron main / renderer 直接：

- 创建虚拟网卡。
- 修改路由。
- 运行 OpenConnect。
- 写系统 DNS。

禁止安装 App 时强制安装 service。

## 6. Merge Blocker

合并到主分支前，必须解决或明确降级说明：

1. Windows one-shot 能力决策：实现 one-shot，或 UI 明确隐藏并提示安装服务。产品目标推荐实现 one-shot。
2. UI 不得隐式尝试不存在的 fallback。
3. macOS helper daemon 入口统一，至少明确迁移策略，推荐本轮完成 `exv-helper --service`。
4. CLI 必须独立于 service 安装，service 未安装时仍能执行 `service install`、`connect --temporary`、`status`。
5. UI / CLI / helper 必须统一状态模型：`service installed`、`helper running`、`backend mode`、`vpn connected`、`last error`。

## 7. 最终用户故事

本轮重构完成后必须满足：

1. 用户不安装服务也能临时连接，授权后 VPN 成功连接，断开后特权 helper 退出。
2. 用户安装后台服务后，连接不再每次弹管理员授权，UI 关闭不影响连接，服务崩溃可由系统拉起。
3. UI 与 CLI 看到同一 helper 状态，任一端 disconnect 后另一端能感知。
4. UI 只展示当前平台真实支持的能力。
5. 首次打开 App 不强制安装服务。
