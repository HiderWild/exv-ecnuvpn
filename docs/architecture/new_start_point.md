# EXV 跨平台 CLI / Desktop / Helper 重构架构规约

## 0. 重构总目标

EXV 需要从“CLI 主导核心逻辑、UI 调用 CLI、服务机制分支化”的形态，重构为：

```text
Core 能力库
    ↑
Privileged Backend / Helper Daemon
    ↑
本地 IPC / RPC 控制协议
    ↑
CLI / Electron Desktop / 其他客户端
```

最终目标是：

```text
exv-core
  只负责 VPN / 虚拟网卡 / 路由 / DNS / native AnyConnect/CSTP / 状态机等核心能力

exv-helper
  特权后端执行体
  既可以作为系统服务长期运行
  也可以作为一次性提权进程临时运行

exv CLI
  普通用户控制端
  可安装/卸载/启动/停止服务
  可连接已运行 helper
  可触发一次性提权 helper

Electron + Vue Desktop
  普通用户 UI
  通过 Electron main 连接 helper
  不直接操作网卡、路由、驱动
  不直接在 renderer 中执行特权逻辑
```

核心原则：

> **服务不是核心逻辑本身，服务只是 `exv-helper` 的一种运行方式。**
> `exv-helper` 可以以 service 模式运行，也可以以 one-shot elevated 模式运行。
> CLI 和 Electron 都是控制端，不是核心执行体。

---

# 1. 必须做成什么

## 1.1 组件边界

目标组件关系如下：

```text
┌──────────────────────────────┐
│        Electron + Vue         │
│  renderer 只负责 UI           │
│  main 负责 IPC / 启动 helper   │
└───────────────┬──────────────┘
                │ Electron IPC
                ▼
┌──────────────────────────────┐
│        Desktop Backend        │
│  BackendResolver              │
│  ServiceManager Client        │
│  Helper RPC Client            │
└───────────────┬──────────────┘
                │ local IPC
                ▼
┌──────────────────────────────┐
│          exv-helper           │
│  --service                    │
│  --oneshot                    │
│  --foreground                 │
└───────────────┬──────────────┘
                │ calls
                ▼
┌──────────────────────────────┐
│          exv-core             │
│  vpn / route / dns / adapter  │
│  native AnyConnect / cleanup  │
│  reconnect / status machine   │
└──────────────────────────────┘
```

CLI 是另一个平级控制端：

```text
┌──────────────────────────────┐
│            exv CLI            │
│  service install/start/stop   │
│  connect/disconnect/status    │
│  temporary connect            │
└───────────────┬──────────────┘
                │ local IPC / bootstrap
                ▼
┌──────────────────────────────┐
│          exv-helper           │
└──────────────────────────────┘
```

---

## 1.2 三种运行模式

`exv-helper` 必须支持三种模式。

### 模式一：service 模式

长期后台服务。

```bash
exv-helper --service
```

用途：

```text
开机自启
断线重连
崩溃后由系统服务管理器拉起
UI 关闭后连接仍可持续
无需每次输入管理员密码
```

平台映射：

```text
Windows:
  Windows Service / SCM
  binary_path = exv-helper.exe --service
  IPC = Named Pipe, e.g. \\.\pipe\exv-helper

macOS:
  launchd / LaunchDaemon
  plist 启动 exv-helper --service
  IPC = Unix Domain Socket, e.g. /var/run/exv-helper.sock
```

注意：macOS 不是 systemd，macOS 对应的是 launchd。Linux 才是 systemd。

---

### 模式二：one-shot elevated 模式

不安装系统服务，但本次连接临时提权。

```bash
exv-helper --oneshot --endpoint <pipe-or-socket> --auth-token <token>
```

用途：

```text
用户不想安装后台服务
每次连接时弹出 UAC / sudo / macOS 管理员授权
连接期间 helper 作为临时特权后端存在
断开连接后清理虚拟网卡 / 路由 / DNS 并退出
```

这个模式是你产品理念里的关键能力：

> **不强制安装服务。安装服务是推荐项，不是使用前提。**

因此，如果 Windows 也要支持“不安装服务也能本次提权连接”，那么 Windows one-shot elevated helper 是 merge blocker。

---

### 模式三：foreground 调试模式

开发者调试使用。

```bash
exv-helper --foreground
```

用途：

```text
本地调试
日志直接输出到终端
不注册服务
不脱离当前 shell
```

---

## 1.3 服务安装与程序安装必须分离

必须明确：

```text
安装 EXV 应用 ≠ 安装系统服务
```

用户安装 App 或解压程序后，磁盘上应该已经存在：

```text
exv
exv-helper
Electron desktop app
必要资源文件
```

但系统服务可以尚未安装。

服务安装只是做这件事：

```text
把已经存在的 exv-helper 注册到系统服务管理器
```

例如：

```text
Windows:
  SCM CreateService 指向 exv-helper.exe --service

macOS:
  写入 /Library/LaunchDaemons/com.ecnu.exv.helper.plist
  plist 指向 /usr/local/bin/exv-helper --service
```

所以 CLI 不能作为“服务安装后的附属物”。CLI 必须在服务安装之前就可用。

---

## 1.4 CLI 的定位

CLI 不再是核心执行者，而是：

```text
用户入口
控制端
service manager client
helper bootstrapper
RPC client
```

CLI 应该支持：

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

其中：

```text
exv connect
  优先连接已运行 service helper
  如果服务未运行但已安装，尝试启动服务
  如果服务未安装，按产品策略触发 one-shot elevated helper

exv connect --temporary
  明确要求本次临时提权连接
  不安装系统服务

exv service install
  只负责注册 exv-helper 为系统服务
  不代表安装 CLI
```

---

## 1.5 Electron Desktop 的定位

Electron Desktop 是用户主入口，但不是特权执行体。

必须采用：

```text
Vue Renderer
  ↓
Electron IPC
  ↓
Electron Main / Desktop Backend
  ↓
BackendResolver
  ↓
exv-helper RPC
  ↓
exv-core
```

Renderer 禁止直接：

```text
执行 PowerShell / osascript
调用 exv-helper
操作路由
操作网卡
读写特权配置
```

Renderer 只发 UI 意图，例如：

```text
connect
disconnect
installService
uninstallService
startService
stopService
getStatus
getLogs
```

具体执行由 Electron main 完成。

---

## 1.6 后端解析器 BackendResolver

需要新增统一的后端选择逻辑。

伪逻辑：

```text
resolveBackend(mode = auto):

1. 检查 service helper 是否正在运行
   如果运行：
     返回 service backend

2. 检查 service 是否已安装
   如果已安装但未运行：
     尝试启动 service
     启动成功后返回 service backend

3. 如果允许 one-shot：
     触发平台提权
     启动 exv-helper --oneshot
     返回 oneshot backend

4. 否则：
     返回 helper_unavailable
     UI/CLI 显示安装服务提示
```

BackendDescriptor 应包含：

```json
{
  "mode": "service | oneshot | foreground",
  "platform": "windows | darwin",
  "transport": "named-pipe | unix-socket",
  "endpoint": "...",
  "auth_required": true,
  "capabilities": {
    "connect": true,
    "disconnect": true,
    "logs": true,
    "reconnect": true,
    "service_install": false,
    "temporary_connect": true
  }
}
```

---

# 2. 必须避免做成什么

## 2.1 避免 Electron 直接嵌 CLI 当核心后端

禁止长期保留这种架构：

```text
Electron UI
  ↓ child_process
exv desktop-rpc vpn.connect
  ↓
直接调用 vpn::start_with_password()
```

这会造成：

```text
UI、CLI、helper 状态不一致
服务模式和临时模式逻辑分叉
Windows/macOS 行为不一致
错误提示无法统一
日志和状态订阅困难
权限边界混乱
```

短期可以保留 `desktop-rpc` 作为兼容桥，但它必须逐步退化为：

```text
desktop-rpc = thin RPC client / bootstrap adapter
```

而不是直接执行核心 VPN 逻辑。

---

## 2.2 避免 UI 假装支持平台没有实现的能力

当前问题：

```text
UI 会尝试 elevated fallback
Windows native policy 写死 no direct fallback
最终仍然只能走 helper/service
```

这是严重的产品行为不一致。

禁止出现：

```text
UI 显示“本次提权连接”
但 native 层其实没有实现
```

必须改成能力驱动：

```text
capabilities.temporary_connect = true/false
```

UI 只能展示当前平台真实支持的能力。

如果 Windows 尚未实现 one-shot，则 UI 必须明确显示：

```text
Windows 当前需要安装 helper service 才能连接。
```

如果产品目标是“Windows 也必须支持不安装服务”，则 Windows one-shot 是阻塞项，不能 release。

---

## 2.3 避免 service 启动 UI

禁止：

```text
service / daemon
  ↓
启动 Electron UI
```

服务是系统后台进程，UI 是用户会话进程。Windows 和 macOS 都存在服务会话隔离，服务直接启动 GUI 会产生权限、桌面会话和安全问题。

正确关系：

```text
用户打开 Electron
Electron 连接 helper
```

---

## 2.4 避免 UI 直接操作网卡、路由、DNS

Electron 不应拥有高权限网络能力。

禁止：

```text
Electron main / renderer
  ↓
直接创建虚拟网卡
直接改路由
直接运行 native VPN 协议生命周期
直接写系统 DNS
```

这些全部归 `exv-helper` 和 `exv-core`。

---

## 2.5 避免服务安装绑定应用安装

禁止：

```text
安装 App 时强制安装 service
```

正确行为：

```text
安装 App:
  放置 exv / exv-helper / desktop app

安装 service:
  用户明确点击“安装后台服务”
  或 CLI 明确执行 exv service install
```

---

# 3. 当前代码状态判断

根据 Codex 返回，当前状态如下。

## 3.1 Windows 当前状态

安装服务后流程：

```text
Electron UI
 -> service.install
 -> Electron main win32 runner
 -> PowerShell RunAs
 -> exv.exe service install
 -> SCM CreateService/ChangeServiceConfig
 -> binary_path = "...\exv-helper.exe" --service
 -> SCM StartService
 -> exv-helper.exe --service
 -> StartServiceCtrlDispatcher / SetServiceStatus(RUNNING)
 -> helper::daemon_main()
 -> named pipe \\.\pipe\exv-helper
```

连接流程：

```text
Electron UI Connect
 -> exv.exe desktop-rpc vpn.connect
 -> app_api
 -> Core / TunnelController
 -> exv-helper service process applies privileged network resources
 -> native AnyConnect/CSTP session
 -> response back to UI
```

问题：

```text
Windows 没有真正实现 one-shot elevated connect
UI 表面尝试 connectElevated
native policy 明确 no direct fallback
所以服务未安装时不能像 macOS 那样本次提权连接
```

结论：

```text
如果产品要求 Windows 不安装服务也能连接：
  Windows one-shot elevated helper 是 merge blocker

如果产品暂时允许 Windows 必须安装服务：
  UI 必须关闭/隐藏“本次提权连接”
  明确提示安装 helper service
```

结合你的产品目标，建议采用前者：**Windows one-shot 是必须补齐的能力。**

---

## 3.2 macOS 当前状态

安装服务后流程：

```text
Electron UI
 -> service.install
 -> osascript with administrator privileges
 -> exv service install
 -> copy/reexec stable /usr/local/bin/exv if needed
 -> write /Library/LaunchDaemons/com.ecnu.exv.helper.plist
 -> launchctl bootstrap system plist
 -> /usr/local/bin/exv __helper-daemon
 -> helper::daemon_main()
 -> Unix socket /var/run/exv-helper.sock
```

连接流程：

```text
Electron UI Connect
 -> exv desktop-rpc vpn.connect
 -> app_api
 -> Core / TunnelController
 -> launchd helper daemon applies privileged network resources
 -> native AnyConnect/CSTP session
 -> response back to UI
```

服务未安装时：

```text
Electron UI Connect
 -> helper_unavailable
 -> osascript admin prompt
 -> exv desktop-rpc vpn.connect {"allow_direct_fallback":true}
 -> darwin try_connect_direct_fallback()
 -> vpn::start_with_password()
```

问题：

```text
macOS 有 direct fallback
但 fallback 不是统一的 exv-helper --oneshot 模式
而是 exv 主程序直接进入特权连接逻辑
```

结论：

```text
短期可以保留
长期应迁移为 exv-helper --oneshot
并让 macOS launchd 也改为启动 exv-helper --service
```

---

# 4. 推荐产品决策

## 4.1 明确产品原则

建议明确写入产品规约：

```text
EXV 不强制用户安装系统服务。

用户可以选择：
1. 临时模式：
   每次连接时进行一次管理员授权，不安装服务。

2. 后台服务模式：
   安装 helper service，获得自动重连、崩溃恢复、开机自启和免重复授权能力。
```

---

## 4.2 Windows 决策

由于产品原则要求“不强制安装服务”，Windows 必须实现：

```text
exv-helper.exe --oneshot
```

不能继续维持：

```cpp
// Windows always requires the helper service; no direct fallback.
return nlohmann::json{};
```

这一逻辑只能作为历史兼容，不能作为目标架构。

---

## 4.3 macOS 决策

macOS 当前可用，但架构不够统一。

建议迁移到：

```text
/usr/local/bin/exv-helper --service
/usr/local/bin/exv-helper --oneshot
```

并逐步废弃：

```text
/usr/local/bin/exv __helper-daemon
exv desktop-rpc vpn.connect {"allow_direct_fallback":true}
```

---

# 5. 目标流程设计

## 5.1 Windows service 模式

```text
Electron UI
 -> service.install
 -> Electron main
 -> UAC RunAs
 -> exv service install
 -> SCM CreateService
 -> binary_path = exv-helper.exe --service
 -> SCM StartService
 -> exv-helper.exe --service
 -> StartServiceCtrlDispatcher
 -> helper::daemon_main()
 -> create named pipe \\.\pipe\exv-helper
 -> UI / CLI connect helper RPC
 -> vpn.connect
 -> exv-core start
```

验收：

```text
服务安装后，UI 连接不再弹 UAC
UI 关闭后连接可继续
服务崩溃后 SCM 可重新拉起
CLI exv status 能连接同一个 helper
```

---

## 5.2 Windows one-shot 模式

目标流程：

```text
Electron UI Connect
 -> BackendResolver detects no service
 -> ask user: 本次临时提权连接？
 -> Electron main generates:
      session_id
      named_pipe_path
      auth_token
 -> PowerShell Start-Process -Verb RunAs:
      exv-helper.exe --oneshot
        --pipe \\.\pipe\exv-oneshot-{session_id}
        --auth-token {token}
 -> elevated exv-helper starts
 -> creates named pipe
 -> UI connects pipe
 -> UI sends auth token
 -> helper accepts session
 -> vpn.connect
 -> exv-core start
 -> disconnect
 -> cleanup routes/adapters/DNS
 -> helper exits
```

注意：

```text
Windows Start-Process -Verb RunAs 不适合依赖 stdout 返回连接信息。
因此 endpoint 和 auth_token 应由 UI 预先生成，并作为参数传给 elevated helper。
```

验收：

```text
完全卸载 service 后
UI 点击连接
弹出 UAC
用户确认后连接成功
系统中没有新注册 Windows Service
连接期间存在 exv-helper elevated process
断开后 helper 退出
路由 / DNS / 虚拟网卡被清理
```

---

## 5.3 macOS service 模式

目标流程：

```text
Electron UI
 -> service.install
 -> osascript with administrator privileges
 -> exv service install
 -> copy stable exv-helper to /usr/local/bin/exv-helper
 -> write /Library/LaunchDaemons/com.ecnu.exv.helper.plist
 -> plist ProgramArguments:
      /usr/local/bin/exv-helper --service
 -> launchctl bootstrap system
 -> exv-helper --service
 -> Unix socket /var/run/exv-helper.sock
 -> UI / CLI connect helper RPC
```

验收：

```text
plist 不再启动 exv __helper-daemon
而是启动 exv-helper --service
UI / CLI 均可连接 /var/run/exv-helper.sock
旧 plist 可迁移或重装
```

---

## 5.4 macOS one-shot 模式

目标流程：

```text
Electron UI Connect
 -> BackendResolver detects no service
 -> ask user: 本次临时提权连接？
 -> Electron main generates:
      socket_path
      auth_token
 -> osascript with administrator privileges:
      exv-helper --oneshot
        --socket /tmp/exv-{uid}-{session_id}.sock
        --auth-token {token}
 -> elevated helper creates Unix socket
 -> chmod/chown socket for current user
 -> UI connects socket
 -> UI sends auth token
 -> vpn.connect
 -> disconnect cleanup
 -> helper exits
```

验收：

```text
不安装 LaunchDaemon
弹出管理员授权
连接成功
断开后 helper 退出
临时 socket 被删除
路由 / DNS / 虚拟网卡清理干净
```

---

# 6. IPC / RPC 设计

## 6.1 Transport

平台默认：

```text
Windows service:
  Named Pipe: \\.\pipe\exv-helper

Windows oneshot:
  Named Pipe: \\.\pipe\exv-oneshot-{session_id}

macOS service:
  Unix Socket: /var/run/exv-helper.sock

macOS oneshot:
  Unix Socket: /tmp/exv-{uid}-{session_id}.sock
```

---

## 6.2 RPC 方法

最小 API 集合：

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

服务管理不建议放在 helper runtime API 里，因为服务未安装时 helper 不存在。

服务管理应由 CLI / Electron main 通过平台 service manager 执行：

```text
service.status
service.install
service.uninstall
service.start
service.stop
```

---

## 6.3 hello/capabilities

所有 helper 启动后必须支持：

```json
{
  "method": "hello",
  "result": {
    "name": "exv-helper",
    "version": "x.y.z",
    "platform": "windows",
    "mode": "service",
    "capabilities": {
      "vpn_connect": true,
      "vpn_disconnect": true,
      "logs": true,
      "events": true,
      "temporary_connect": true,
      "service_mode": true
    }
  }
}
```

UI 不得猜能力，必须读 capabilities。

---

## 6.4 安全要求

必须满足：

```text
1. oneshot endpoint 必须带随机 session_id
2. oneshot 必须带 auth_token
3. helper 收到第一条请求必须验证 auth_token
4. service pipe/socket 必须有合理 ACL / 文件权限
5. helper 不接受任意 shell 命令
6. helper 只接受结构化 RPC action
7. 所有来自 UI/CLI 的配置都必须校验
8. privileged helper 不信任 renderer 输入
```

Windows Named Pipe：

```text
service pipe:
  ACL 限制为当前用户组 / Administrators / SYSTEM，具体按产品需求决定

oneshot pipe:
  endpoint 随机化
  auth token 必须验证
```

macOS Unix socket：

```text
service socket:
  /var/run/exv-helper.sock
  权限受控

oneshot socket:
  /tmp/exv-{uid}-{session_id}.sock
  chmod 0600 或等价权限
  断开后删除
```

---

# 7. 任务拆解

下面是适合多智能体执行的任务树。

---

## Epic A：架构决策与规约冻结

### A1. 冻结产品运行模式决策

目标：

```text
明确 EXV 支持 service 模式和 one-shot elevated 模式。
明确 Windows 也必须支持 one-shot，除非 release 明确声明 Windows service-only。
```

实施内容：

```text
写入 docs/architecture/runtime-modes.md
定义 service / oneshot / foreground 三种模式
定义各模式用户体验差异
定义平台能力矩阵
```

验收标准：

```text
文档明确说明：
- 安装 App 不等于安装服务
- 服务安装是可选项
- CLI 和 UI 都应支持临时模式
- Windows one-shot 是目标能力
```

依赖：

```text
无
```

并行性：

```text
可最先执行，阻塞后续所有实现任务
```

---

### A2. 定义组件边界文档

目标：

```text
冻结 exv / exv-helper / exv-core / desktop 的职责边界。
```

实施内容：

```text
新增 docs/architecture/components.md

明确：
- exv-core 不依赖 UI/CLI
- exv-helper 是唯一特权执行体
- exv CLI 是控制端
- Electron 是控制端
- service manager 是安装/卸载服务的工具层
```

验收标准：

```text
文档中包含：
- 正向架构图
- 禁止架构图
- 各组件输入/输出
- 哪些组件可提权，哪些不可提权
```

依赖：

```text
A1
```

并行性：

```text
可与 A3 并行
```

---

### A3. 定义 RPC 协议草案

目标：

```text
冻结 helper runtime API 的最小协议。
```

实施内容：

```text
新增 docs/architecture/helper-rpc.md

定义：
- hello
- status
- vpn.connect
- vpn.disconnect
- logs.tail
- events.subscribe
- helper.shutdown
```

验收标准：

```text
Windows/macOS helper 均可按同一协议实现。
UI/CLI 不再依赖平台私有返回结构。
```

依赖：

```text
A1
```

并行性：

```text
可与 A2 并行
```

---

## Epic B：合并前代码盘点与分支收敛

### B1. 建立平台差异清单

目标：

```text
明确 Windows/macOS 当前代码差异，避免盲目 merge。
```

实施内容：

```text
列出：
- service install 实现差异
- helper daemon 入口差异
- IPC transport 差异
- direct fallback 差异
- desktop-rpc 差异
- build/package 差异
```

验收标准：

```text
产出 docs/merge/platform-diff.md
必须包含当前 Codex 发现的问题：
- Windows UI 尝试 elevated fallback 但 native 未实现
- macOS 有 direct fallback
- macOS service 仍用 exv __helper-daemon
- Windows 已拆 exv-helper.exe --service
```

依赖：

```text
A1
```

并行性：

```text
可与 Epic C 的接口设计并行
```

---

### B2. 建立平台能力矩阵

目标：

```text
让 UI/CLI 通过能力矩阵判断功能，而不是猜平台行为。
```

示例：

```json
{
  "windows": {
    "service_mode": true,
    "oneshot_mode": false,
    "direct_fallback": false,
    "helper_binary": true
  },
  "darwin": {
    "service_mode": true,
    "oneshot_mode": true,
    "direct_fallback": true,
    "helper_binary": false
  }
}
```

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

验收标准：

```text
UI 不再硬编码“尝试 fallback”
而是读取 capability。
Windows 未实现 oneshot 前，UI 不展示临时连接。
Windows 实现 oneshot 后，capability 改为 true。
```

依赖：

```text
A1, A3
```

并行性：

```text
可与 UI 任务并行
```

---

## Epic C：exv-core 抽离

### C1. 抽离核心 VPN 能力库

目标：

```text
把 vpn start/stop/status/cleanup 从 CLI/UI/helper 代码中抽离到 exv-core。
```

实施内容：

```text
建立 core 层接口：

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

验收标准：

```text
CLI 不再直接散落调用平台网络逻辑。
helper 可以调用同一套 core。
macOS 和 Windows 的平台差异被封装到 platform adapter。
```

依赖：

```text
A2
```

并行性：

```text
可由 core agent 独立完成，但会与 Windows/macOS 平台文件冲突，需要锁定 core 目录。
```

---

### C2. 定义平台适配接口

目标：

```text
把 Windows/macOS 差异封进 platform adapter。
```

接口建议：

```text
IAdapterManager
IRouteManager
IDnsManager
IVpnProcessRunner
IPrivilegeContext
IServiceManager
IIpcTransport
```

验收标准：

```text
core 调用抽象接口
platform/windows 实现 Windows 行为
platform/darwin 实现 macOS 行为
不在 UI/CLI 中散落平台网络逻辑
```

依赖：

```text
C1
```

并行性：

```text
Windows agent 和 macOS agent 可并行实现各自 adapter。
```

---

## Epic D：统一 exv-helper

### D1. 建立 exv-helper 统一入口

目标：

```text
让 exv-helper 成为跨平台统一特权执行体。
```

命令形式：

```bash
exv-helper --service
exv-helper --oneshot --endpoint <endpoint> --auth-token <token>
exv-helper --foreground
```

验收标准：

```text
Windows 保持 exv-helper.exe --service 可用
macOS 新增 exv-helper 可执行文件
helper::daemon_main() 成为共同入口
```

依赖：

```text
A2, A3, C1
```

并行性：

```text
Windows/macOS 可以并行，但 daemon_main 接口需要先冻结。
```

---

### D2. macOS 从 exv __helper-daemon 迁移到 exv-helper --service

目标：

```text
macOS launchd 不再启动 exv 主程序内部隐藏模式。
```

实施内容：

```text
修改 service install：
旧：
  /usr/local/bin/exv __helper-daemon

新：
  /usr/local/bin/exv-helper --service
```

验收标准：

```text
新安装的 plist 指向 exv-helper --service
旧 plist 可被卸载重装或迁移
service 启动后仍暴露 /var/run/exv-helper.sock
```

依赖：

```text
D1
```

并行性：

```text
macOS agent 独立执行
```

---

### D3. 保留兼容 shim

目标：

```text
避免老版本用户升级时直接断裂。
```

实施内容：

```text
exv __helper-daemon 可以短期保留，但内部只转发/提示使用 exv-helper。
```

验收标准：

```text
旧命令不会崩溃
日志明确提示 deprecated
新安装不再生成旧 plist
```

依赖：

```text
D2
```

并行性：

```text
可与 UI 更新并行
```

---

## Epic E：Windows one-shot elevated helper

这是本次最关键的补齐任务。

### E1. 设计 Windows one-shot bootstrap

目标：

```text
实现 Windows 未安装 service 时，本次 UAC 提权连接。
```

实施内容：

```text
Electron main / CLI 生成：
- session_id
- pipe path
- auth_token

通过 UAC 启动：
exv-helper.exe --oneshot --pipe <pipe> --auth-token <token>
```

注意：

```text
不要依赖 elevated 进程 stdout 返回 pipe 信息。
因为 Start-Process -Verb RunAs 不适合做 stdout IPC。
```

验收标准：

```text
可以在服务未安装状态下启动 elevated exv-helper。
UI/CLI 能连接它创建的 named pipe。
```

依赖：

```text
D1, A3
```

并行性：

```text
Windows agent 独立执行
```

---

### E2. 实现 Windows oneshot named pipe server

目标：

```text
exv-helper --oneshot 创建临时 named pipe 并提供 RPC。
```

实施内容：

```text
pipe name:
  \\.\pipe\exv-oneshot-{session_id}

启动后：
  create pipe
  wait client
  validate auth token
  serve RPC
  after disconnect/shutdown cleanup and exit
```

验收标准：

```text
服务未安装时：
- UI connect
- 弹 UAC
- helper 启动
- pipe 建立
- auth 成功
- vpn.connect 成功
```

依赖：

```text
E1
```

并行性：

```text
可与 UI capability 改造并行，但最终集成依赖 UI。
```

---

### E3. 移除 Windows no direct fallback 策略

目标：

```text
不再写死 Windows always requires helper service。
```

不是改成“直接调用 core”，而是改成：

```text
Windows supports oneshot helper backend.
```

验收标准：

```text
原 native policy 不再返回空 fallback
而是通过 BackendResolver 启动/连接 oneshot helper
```

依赖：

```text
E2
```

并行性：

```text
不可提前做，否则 UI 会调用未完成能力。
```

---

## Epic F：macOS one-shot helper 化

### F1. 把 macOS direct fallback 改为 exv-helper --oneshot

目标：

```text
废弃 exv desktop-rpc 直接 vpn::start_with_password 的临时提权路径。
```

实施内容：

```text
旧：
osascript -> exv desktop-rpc vpn.connect allow_direct_fallback -> vpn::start_with_password

新：
osascript -> exv-helper --oneshot --socket <path> --auth-token <token>
UI/CLI -> socket RPC -> vpn.connect
```

验收标准：

```text
macOS 服务未安装时仍可临时连接
但连接执行体是 exv-helper --oneshot
不是 exv desktop-rpc 直接执行 core
```

依赖：

```text
D1, A3
```

并行性：

```text
macOS agent 可与 Windows one-shot 并行
```

---

### F2. macOS 临时 socket 权限与清理

目标：

```text
保证 root 启动的 oneshot helper 创建的 socket 可被当前用户连接且安全。
```

实施内容：

```text
socket path:
  /tmp/exv-{uid}-{session_id}.sock

创建后：
  chown 到目标用户
  chmod 0600 或等价控制
  验证 auth token
  退出时删除 socket
```

验收标准：

```text
普通用户 UI 能连接 socket
其他用户不能随意连接
helper 退出后 socket 文件不存在
```

依赖：

```text
F1
```

并行性：

```text
可与 macOS service 迁移并行
```

---

## Epic G：BackendResolver 统一实现

### G1. 实现 shared BackendResolver

目标：

```text
CLI 和 Electron main 使用同一套后端选择逻辑。
```

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

验收标准：

```text
CLI 和 Electron main 不各自实现一套平台判断。
Windows/macOS 都通过 BackendResolver 决定连接谁。
```

依赖：

```text
A3, D1
```

并行性：

```text
可与 UI 改造并行，但最终需集成。
```

---

### G2. 统一 helper_unavailable 错误语义

目标：

```text
消除当前 helper_unavailable 后 UI 盲目尝试 fallback 的行为。
```

新错误分类：

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

验收标准：

```text
UI 可以根据错误显示准确提示。
Windows 未实现 oneshot 时，不会误导用户。
```

依赖：

```text
G1
```

并行性：

```text
可与 UI 文案任务并行
```

---

## Epic H：CLI 重构

### H1. CLI connect 改为 helper client

目标：

```text
CLI 不直接执行 VPN 核心逻辑。
```

实施内容：

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

验收标准：

```text
服务模式和临时模式下 CLI 行为一致。
CLI 与 UI 看到同一 helper 状态。
```

依赖：

```text
G1, D1
```

并行性：

```text
CLI agent 可独立开发，需 mock helper。
```

---

### H2. CLI service 命令重构

目标：

```text
service install/uninstall/start/stop 只管理系统服务，不执行连接。
```

验收标准：

```text
exv service install:
  Windows 注册 exv-helper.exe --service
  macOS 注册 exv-helper --service

exv service uninstall:
  删除系统服务
  不删除 CLI 本体
```

依赖：

```text
D1, D2
```

并行性：

```text
Windows/macOS service manager 可并行。
```

---

## Epic I：Electron UI 重构

### I1. UI 状态模型重构

目标：

```text
UI 明确区分四种状态。
```

必须区分：

```text
App/UI 是否打开
Service 是否安装
Helper 是否运行
VPN 是否连接
```

不能混为一谈。

UI 状态示例：

```text
服务未安装：
  显示：
    - 本次临时连接
    - 安装后台服务

服务已安装但未运行：
  显示：
    - 启动服务
    - 本次临时连接

服务运行中：
  显示：
    - 连接
    - 断开
    - 查看日志
```

验收标准：

```text
UI 不再把“打开 UI”和“启动连接”绑定。
UI 不再把“安装服务”和“安装应用”绑定。
```

依赖：

```text
A1, G2
```

并行性：

```text
UI agent 可先用 mock capabilities 开发。
```

---

### I2. UI capability-driven 行为

目标：

```text
UI 不再硬编码平台判断。
```

实施内容：

```text
读取 capabilities：
- service_mode
- oneshot_mode
- service_installed
- helper_running
```

验收标准：

```text
Windows oneshot 未完成时，不显示“本次提权连接”。
Windows oneshot 完成后，自动显示。
macOS helper 迁移前后 UI 不需要大改。
```

依赖：

```text
B2, G1
```

并行性：

```text
可与 Windows/macOS 后端实现并行。
```

---

### I3. connectElevated 语义重命名

当前：

```text
vpn.connectElevated
```

容易让人误解为“直接提权执行连接”。

建议改为：

```text
vpn.connectTemporary
```

或者：

```text
vpn.connectOneshot
```

语义：

```text
启动/连接 one-shot elevated helper，然后通过 RPC 发起连接。
```

验收标准：

```text
代码中不再出现 allow_direct_fallback 作为目标路径。
如果保留，也只能作为 deprecated compatibility。
```

依赖：

```text
G1, F1, E2
```

并行性：

```text
UI agent 和 backend agent 协调修改。
```

---

## Epic J：测试与验收

### J1. Windows service 模式测试

验收脚本：

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

```text
无 UAC 重复弹窗
named pipe 可连接
helper 状态正确
路由/DNS 清理正确
```

---

### J2. Windows one-shot 模式测试

验收脚本：

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

```text
不安装服务也能连接
失败时错误提示准确
取消 UAC 时 UI 显示 elevation_denied
```

---

### J3. macOS service 模式测试

验收脚本：

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

```text
不再依赖 exv __helper-daemon
socket 正常创建
权限正确
```

---

### J4. macOS one-shot 模式测试

验收脚本：

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

```text
无 service 安装
临时连接成功
清理完整
```

---

# 8. 任务依赖与并行策略

## 8.1 总体依赖图

```text
A1 架构决策
  ↓
A2 组件边界 ──→ C1 core 抽离 ──→ D1 helper 统一入口
  ↓                   ↓                  ↓
A3 RPC 协议 ───────→ C2 平台适配 ─────→ E/F oneshot 实现
  ↓                                      ↓
B2 capability 矩阵 ───────────────→ G BackendResolver
                                           ↓
                         H CLI 重构 ──────┼────── I UI 重构
                                           ↓
                                      J 测试验收
```

---

## 8.2 可并行任务

可以并行：

```text
Agent 1:
  A1/A2/A3 文档和协议冻结

Agent 2:
  C1/C2 core 抽离

Agent 3:
  Windows service/oneshot helper

Agent 4:
  macOS exv-helper 迁移/oneshot helper

Agent 5:
  Electron UI 状态模型/capability UI

Agent 6:
  CLI command 重构

Agent 7:
  测试脚本/验收用例/CI
```

但是必须注意：

```text
UI agent 不得自定义后端协议
CLI agent 不得自定义另一套 resolver
Windows/macOS agent 不得修改 core 公共 API，除非通过架构负责人批准
```

---

## 8.3 冲突高发目录建议

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

---

# 9. Merge blocker 清单

在合并到主分支前，必须解决或明确降级说明。

## Blocker 1：Windows one-shot 能力决策

必须二选一：

```text
A. 实现 Windows one-shot elevated helper
   则 UI 可展示“本次临时连接”。

B. 暂不实现
   则 UI 必须隐藏该能力，并明确提示 Windows 需要安装 helper service。
```

根据产品目标，推荐 A。

---

## Blocker 2：UI 不得隐式尝试不存在的 fallback

当前行为必须修复：

```text
helper_unavailable
 -> connectElevated
 -> Windows native no direct fallback
```

目标：

```text
helper_unavailable
 -> read capabilities
 -> if oneshot supported: start oneshot
 -> else show install service prompt
```

---

## Blocker 3：macOS helper daemon 入口统一

至少需要明确迁移策略：

```text
短期：
  exv __helper-daemon 保留兼容

长期：
  LaunchDaemon 启动 exv-helper --service
```

建议在本轮重构中完成长期方案。

---

## Blocker 4：CLI 独立于 service 安装

必须保证：

```text
service 未安装时，exv CLI 仍可执行：
- exv service install
- exv connect --temporary
- exv status
```

---

## Blocker 5：状态模型统一

UI / CLI / helper 必须看到一致状态：

```text
service installed
helper running
backend mode
vpn connected
last error
```

不能出现：

```text
UI 以为连接中
helper 已退出

CLI 以为服务没运行
UI 仍显示连接成功

Windows 显示临时连接
实际只能安装服务
```

---

# 10. 最终验收标准

本轮重构完成后，应满足以下用户故事。

## 用户故事 1：不安装服务，临时连接

```text
作为用户，我可以不安装后台服务。
我点击“本次临时连接”。
系统弹出管理员授权。
授权后 VPN 成功连接。
断开后后台特权进程退出。
```

Windows/macOS 都应成立。

---

## 用户故事 2：安装后台服务后连接

```text
作为长期用户，我可以安装 helper service。
安装后连接不再每次弹管理员授权。
服务可以后台运行。
UI 关闭不影响连接。
服务崩溃后系统可拉起。
```

Windows/macOS 都应成立。

---

## 用户故事 3：CLI 与 UI 一致

```text
我用 UI 连接后，exv status 能看到连接状态。
我用 CLI disconnect 后，UI 能看到断开状态。
```

---

## 用户故事 4：能力真实展示

```text
如果当前平台不支持某能力，UI 不展示假按钮。
如果支持，UI 展示并能成功执行。
```

---

## 用户故事 5：服务不是强制项

```text
首次打开 App 时，不应强制安装服务。
UI 可以推荐安装服务，但必须允许临时连接。
```

---

# 11. 给智能体团队的最终指令摘要

可以把下面这段直接塞给负责重构的智能体：

```text
本项目目标不是把 CLI 嵌入 Electron，也不是让 UI 直接操作 VPN。
目标架构是：exv-core 提供核心能力，exv-helper 作为唯一特权执行体，CLI 和 Electron 都作为控制端通过本地 RPC 调用 helper。

exv-helper 必须支持 --service 和 --oneshot 两种正式运行模式。
service 模式用于长期后台服务；oneshot 模式用于不安装服务时的本次提权连接。

安装应用不等于安装服务。
CLI 必须独立存在，并能够在 service 未安装时执行 service install 或 temporary connect。

Windows 当前没有真正实现 one-shot elevated connect，这是目标架构下的 merge blocker。
如果产品目标是不强制安装服务，则 Windows 必须补齐 exv-helper.exe --oneshot。
UI 不允许展示或隐式尝试平台未实现的 fallback。

macOS 当前 direct fallback 可工作，但长期必须迁移为 exv-helper --oneshot。
macOS LaunchDaemon 也应从 exv __helper-daemon 迁移为 exv-helper --service。

所有 UI 行为必须 capability-driven。
不要根据平台字符串猜能力。
helper hello/status 必须返回 mode、transport、capabilities。

禁止长期保留 Electron -> CLI -> direct core connect 的结构。
desktop-rpc 可以短期保留为兼容入口，但必须退化为 helper RPC client / backend bootstrapper。
```

核心一句话：

> **EXV 的核心不是 CLI，也不是 Electron，而是可被 service 或 one-shot 托管的 privileged helper backend。CLI 和 UI 只是两个平级控制端。**
