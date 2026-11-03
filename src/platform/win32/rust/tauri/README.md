# EXV Tauri 桌面 UI（Phase 4 · P4-a 骨架）

独立 requirement：`vpn-rust-tauri-desktop-ui`（spec：不塞进 Common workspace）。
前端重写（O2）：不复用 C++ `webui/` 代码，仅参考其功能清单。

## 结构

```
src/platform/win32/rust/tauri/        # 独立 workspace（独立 Cargo.lock）
├── Cargo.toml                        # workspace 根（members = ["app"]）
├── app/                              # Tauri 应用 crate（tauri v2）
│   ├── tauri.conf.json               # devUrl: localhost:1420；frontendDist: ../frontend/dist
│   ├── capabilities/default.json     # 权限：core IPC + event + window + log
│   ├── build.rs
│   ├── icons/                        # 占位图标（P4 正式版需替换品牌图标）
│   └── src/
│       ├── main.rs / lib.rs          # Builder：命令注册、托盘、窗口生命周期
│       ├── lifecycle.rs              # 托盘最小化不退出；彻底退出→notify_core_shutdown（O3 接缝）
│       └── kernel/                   # UI↔core 语义层（mirror wire 契约）
│           ├── state.rs              # RuntimeSnapshot/RuntimeEvent/ConnectPhase/OperationReply
│           ├── logs.rs               # LogEvent/LogChunk
│           ├── commands.rs           # Tauri Command：connect/stop/snapshot/logs_list/config_*/respond_interaction
│           ├── events.rs             # Event 名 + emit 辅助（exv://status|logs|interaction）
│           ├── client.rs             # CoreClient 接缝 + CoreState（P4-b 接真实 gRPC 通道）
│           └── error.rs              # AppError（not_wired / core_unreachable / internal）
└── frontend/                         # Vue3 + Vite + TS（前端重写）
    ├── package.json / vite.config.ts / tsconfig.json / index.html
    └── src/
        ├── main.ts / App.vue         # 侧边栏 + 三页路由（连接/日志/设置）
        ├── lib/ipc.ts                # 类型化 invoke()/listen() 封装（mirror Rust 契约）
        └── pages/                    # ConnectPage / LogsPage / SettingsPage
```

## 构建 / 运行

宿主负责提供 Rust、sccache、Node.js 与前端依赖。以下命令在候选工作树根目录执行；
Rust 产物统一放到主仓 target。当前启用 custom-protocol，Rust 检查与单元测试之前
同样需要生成真实前端资产。

```powershell
$gitCommonDir = git rev-parse --path-format=absolute --git-common-dir
$env:CARGO_TARGET_DIR = Join-Path (Split-Path $gitCommonDir) 'target'
$env:RUSTC_WRAPPER = 'sccache'
$env:CARGO_INCREMENTAL = '0'
./scripts/clear-repository-build-artifacts.ps1 -KeepLatestInstaller
npm.cmd --prefix src/platform/win32/rust/tauri/frontend run build
cargo check --manifest-path src/platform/win32/rust/tauri/Cargo.toml -p exv-ui -j 1
cargo test --manifest-path src/platform/win32/rust/tauri/Cargo.toml -p exv-ui -j 1
./scripts/clear-repository-build-artifacts.ps1 -KeepLatestInstaller
```

### Windows 生产包构建与资源门禁

生产包统一通过仓库 `scripts/assemble-runtime-trio.ps1` 调用 Windows `tauri.cmd build --no-bundle`。前端保持 `app/tauri.conf.json` 的相对目录 `../frontend/dist`，由 CLI 的 `beforeBuildCommand` 生成真实资产。所有 Rust 构建使用 sccache、incremental=0、单任务和主仓唯一 target；不得用 Windows 盘符绝对路径覆盖 `frontendDist`。

在已隔离的候选工作树根目录执行，前端依赖需已由宿主提供：

```powershell
./scripts/clear-repository-build-artifacts.ps1 -KeepLatestInstaller
./scripts/assemble-runtime-trio.ps1
./scripts/build-rust-setup.ps1 -Jobs 1
./scripts/package-rust-setup.ps1 -KeepInstallers 1
# 完成载荷与源文件核验后立即清理，只保留共享目录最新安装包。
./scripts/clear-repository-build-artifacts.ps1 -KeepLatestInstaller
```

未传 `-Version` 时安装器版本取自 `app/tauri.conf.json`。载荷中的 `exv-ui.exe` 必须与主仓 `target/release/exv-ui.exe` 的 SHA256 一致；不得只依据文件名或时间戳判定最新。

应用 `build.rs` 的资源门禁使用 Tauri 实际配置解析，包含平台配置与 `TAURI_CONFIG` 合并结果。Windows 页面必须来自相对 `Directory`，拒绝外部 URL、绝对路径、文件列表和缺省资源。实际嵌入模式要求目录与 `index.html` 存在；开发服务器模式不要求预先生成前端输出。当前启用 `custom-protocol` 的 Rust 检查和单元测试同样需要先生成前端资产，不通过虚假入口文件绕过；无需新增 feature。

Windows 盘符字符串可能先被 Tauri 解析为外部 URL，此时框架不嵌入资产，编译和打包仍可能成功。载荷 CRC 和源哈希仅证明包装一致。交付必须在清除 `target` 与 `dist` 后启动包内程序，并在实际安装位置图形观察正常产品页面；不得回填构建目录，也不得把页面 `ERR_FILE_NOT_FOUND` 的包用于继续业务验收。安装包只保留同级共享目录最新一份，不长期保留工作树运行副本。这一 Windows 约束不改变 Darwin 已有宿主资源路径。

## P4-a 阶段现状（诚实边界）

- Command/Event 类型与接缝就位，签名即契约（mirror `proto/exv/v1/` wire）。
- 所有 Command 后端经 `CoreClient` 返回 `AppError::NotWired`（P4-b 前占位）。
- WatchEvents/StreamLogs 订阅为 Event 名 + emit 辅助（接缝），未接真实推送源。
- 托盘：最小化不退出（CloseRequested → hide）；「退出」→ `notify_core_shutdown`（O3 接缝，当前仅日志）。
- 图标为占位（cyan 环），正式版需替换。

## P4-a 验证结果（2026-08-17）

- `cargo check -p exv-ui`：通过，无错误无警告。
- `npm run build`（vue-tsc + vite）：通过，产物 `frontend/dist/`（72 kB JS / 4.6 kB CSS）。
- `cargo build -p exv-ui`：通过，产出 `target/debug/exv-ui.exe`（14.7 MB，debug）。
  验证完成后已删除该 target 目录释放 D: 空间（共享 worktree，D: 曾降至 423M 空闲）；
  重建约需 5–10 分钟（cargo registry 缓存位于 C: 已热身）。
- 环境：cargo 1.96.0（完整路径）；node v24 / npm 11；C: 78G 空闲（cargo cache）；
  D: 空间紧张（win32 rust + root 两个 target 共 20G+；tauri target 约 2.9G，构建时需预留）。

## P4-b IPC 接线说明（已实现；2026-08-17）

架构前提（计划 §1）：core 是独立进程（普通用户 token · 纯协调层 · 语义网关），
UI 不直连 engine。Tauri host 进程是 core 的唯一 UI 入口。

通道：**Tauri Command → host Rust → core 的 KernelControl gRPC 端点（Named Pipe）**。

1. **core 进程形态**：core 独立进程，暴露 `KernelControl` 服务（proto
   `kernel_control.proto`：Connect/RespondInteraction/Stop/Reconcile/GetOperation/
   GetSnapshot/WatchEvents）。其传输层在
   `crates/exv-vpn-win32-host/src/kernel_control_transport.rs` / `grpc_transport.rs`。
2. **传输选型（Windows）**：**named pipe**（`\\.\pipe\exv-core-<ui_pid>`，
   mutually-authenticated，与 host↔engine 的 pipe 认证同构）。
   - UI 侧 client 拨号 + server 身份验证在 `app/src/kernel/core_transport.rs`
     （镜像 host `grpc_transport.rs`：`dial_core_pipe_with_retry` +
     `verify_core_server_pipe` + `connect_core_channel`）。
   - core 侧验证 UI peer（pid + user SID + account）由 host
     `kernel_control_transport::verify_ui_peer` 完成（P5 落 host main 时接线）。
3. **生命周期（O3 强绑定）**：core 由 **Tauri 宿主进程 spawn**（`core_process.rs`，
   非特权直接拉起——core 是普通用户 token）。UI 彻底退出（托盘「退出」）→
   `notify_core_shutdown`（`lifecycle.rs`）：中止事件订阅 → `app.exit(0)` →
   进程 teardown drop channel → 控制面管道关闭 → core `serve_task` resolve →
   `shutdown_core` 有序停机（engine StopTunnel → engine 终止）→ core 退出。
   **core 由 pipe-close 感知自行退出**；UI 侧 `CoreChild` 不做 Drop 强杀（避免打断
   有序停机），`terminate` 为挂死兜底。
4. **接线点**：
   - `kernel/client.rs`：`CoreClient` 方法体改为真实 tonic 调用；
     `CoreHandle::Dialed { channel, core_peer }`。
   - `kernel/wire.rs`：wire → UI 镜像类型机械映射 + 请求组装（connect/stop/
     respond_interaction）。
   - `kernel/events.rs`：`spawn_subscriptions` 拉起 WatchEvents 订阅 task
     （`stream.next()` → `emit_status`，断线退避重连 + resume tick）。
   - `kernel/bootstrap.rs`：Tauri setup 时 spawn core → 拨号 → 管理状态 → 挂订阅。
   - `commands.rs`：命令签名不变，仅 async 化（`State` 仍同步可用）。
5. **依赖**：`app/Cargo.toml` 增补 `tonic`/`http`/`hyper-util`/`tower`/`uuid`/
   `sha2`/`windows`，并**复用 Root workspace 的 `exv-vpn-wire`**（path dependency）
   作为 `KernelControlClient` + wire 消息类型源——不复制 proto、不重生成，避免
   wire 契约分叉。tonic 版本与核心 workspace 一致（0.14.x）。

### P4-b 诚实边界（wire 缺口）

`KernelControl` 冻结 wire 只有 Connect/RespondInteraction/Stop/Reconcile/GetOperation/
GetSnapshot/WatchEvents。以下命令无 wire 端点，保留 typed `NotWired`（不假装完成，
proto 变更须协调者评估）：
- `logs_list`（`logs.list`/`logs.clear` 在 host `log_control.rs` 为进程内方法，不在
  KernelControl proto）；
- `config_get` / `config_set`（core config 契约不在 wire 上）。
- `exv://logs` 事件（StreamLogs 是 HelperControl 的 engine→core 通道，core 未向 UI
  暴露）——emit seam 保留，前端仍监听该名，接线后生效。

## P5-c 统计展示说明（已实现；2026-08-17）

连接页展示 rx/tx 速度（bytes/s，参考 C++ `formatTrafficRate` 逻辑）、累计流量
（rx/tx bytes）、延迟与统计阶段；连接状态（phase/connected）来自 `WatchEvents`
`RuntimeSnapshot`（P4-b 已真实接线）。

**P5-c wire 缺口（数据通道）**：统计数值源在 host `EventBus` 统计 lane
（`publish_stats`/`subscribe_stats`/`current_stats`，host `kernel_control_service.rs`；
P5-b core 侧归一化：累计字节增量 / 时间间隔的速度 + 累计流量透传）。但 `KernelControl`
冻结 wire 无统计 RPC、`RuntimeSnapshot`（proto `common.proto`）无统计字段——**UI 目前
无法经 wire 取到统计**。tauri 侧已备好完整管道与 typed `NotWired` seam：
- `kernel/stats.rs`：UI 侧 `RuntimeStats` 镜像（serde，与 host `RuntimeStats` 字段一致）；
- `stats` 命令（`CoreClient::stats`）+ `exv://stats` 事件（`emit_stats`）+ `onStats`
  前端监听 + `CoreState.last_stats` 缓存——wire 源落地后仅需把 seam 换为真实调用；
- 前端 `ConnectPage.vue` 统计面板已完整渲染；seam 未接线期显示「统计通道待接线
  （P5-c）」占位，不假装有数据。

host/proto 变更须协调者评估（建议见 `app/src/kernel/stats.rs` 模块文档：A. 扩展
`RuntimeSnapshot` 增 `stats` 字段；B. `KernelControl` 增 `GetStats`/`StreamStats` RPC）。
给 P6 验收：统计数值通道接通 = 把 `CoreClient::stats` 从 seam 换为真实调用 + 前端
`onStats` 订阅生效；速度/流量格式化逻辑与显示已完成并随 seam 就位。

### 构建环境注意（P4-b 起）

D: 磁盘紧张时，Tauri 构建和链接临时文件仍必须留在当前工作树内，不能写入 C: 系统 Temp：
```bash
export PATH="/c/Users/user/.rustup/toolchains/1.96.0-x86_64-pc-windows-msvc/bin:$PATH"
cd src/platform/win32/rust/tauri
CARGO_TARGET_DIR="$(pwd)/../.build-tmp/tauri-target" cargo check -p exv-ui
CARGO_TARGET_DIR="$(pwd)/../.build-tmp/tauri-target" cargo test -p exv-ui
```
Cargo 的 `TMP`/`TEMP` 由上级 `rust/.cargo/config.toml` 以 `relative = true` 和
`force = true` 固定到 `rust/.build-tmp`，即使启动 Cargo 的父进程已设置这两个变量亦然；
构建产物清理脚本会统一清除该目录中的可再生内容。

## 与核心 runtime workspace 的关系

- 独立 workspace、独立 Cargo.lock、独立依赖治理（计划 §4 风险项：需单独 Cargo.toml）。
- 不修改 `src/platform/win32/rust/Cargo.toml`（其 members 仅 `crates/*`，tauri/ 不加入）。
- 不共享 `src/platform/win32/rust/.cargo/config.toml`（TMP/TEMP workaround 适用目录树，
  本目录位于其下，自动继承，勿删除）。
- 未来如需复用 core 类型：path dependency 或经 gRPC 通道，P4-b 决定。
