# macOS Desktop-First Full-UI Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `macos` 分支上，把当前仍以 CLI/browser-WebUI 为主的实现推进到真正的桌面优先产品形态：用户通过 Electron 图形界面即可完成 launchd helper 安装、一次性授权连接、状态查看、断开、配置修改和错误恢复；同时保持 macOS 上的 route cleanup、route-ready 与 helper 生命周期一致可靠。

**Architecture:** 优先复用 `windows` 分支已经验证过的共享 desktop-first renderer contract 与页面结构；macOS 专属部分聚焦在 launchd helper、`osascript` 一次性提权、Unix socket helper IPC、以及断连/退出/异常路径上的 route cleanup 正确性。浏览器 WebUI 继续保留，但降级为兼容入口。

**Tech Stack:** C++17, CMake, Electron 39, Vue 3, Pinia, Vue Router, Vite, macOS `launchd`, `osascript` administrator privileges, Unix domain socket IPC, CommonCrypto, DMG packaging.

---

## Branch Baseline

当前 `macos` 工作树是干净分支，但不是没有基础：

- 现有代码已经有 macOS helper IPC 与 launchd 管理基础，`src/helper_daemon_mac.cpp` 使用 Unix socket + `getpeereid` 验证客户端。
- `src/helper.cpp`、`src/tunnel.cpp` 已经持有 route-ready、route cleanup 与 helper stop cleanup 逻辑，这正是 mac 分支名 `fix-mac-route-cleanup` 所指向的重点。
- Electron、Vue、desktop-rpc、桌面壳层与 Windows 分支共享大量代码结构，可以被复用而不必重写。
- 但当前基线整体仍偏向 CLI/browser-WebUI 叙事，尚未把图形界面收敛成主入口，也没有完成 macOS 专属的一次性授权与路由清理闭环验证。

因此，macOS 分支的计划不是简单复制 Windows，而是“复用共享闭环 + 补齐 macOS 系统差异”。

## Implementation Goal

最终要达到的 macOS 用户路径如下：

1. 用户启动桌面端后，默认通过 GUI 完成连接与配置，不需要浏览器，也不需要终端。
2. 已安装 launchd helper 时，连接/断开/状态读取全部通过稳定的 helper path 完成。
3. 未安装 helper 时，Dashboard 能推荐安装服务，同时支持用 `osascript` 触发一次性管理员授权完成临时连接。
4. direct/elevated 会话结束、helper 停止、异常断连后，route cleanup 必须稳定执行，不能留下污染路由。
5. 文档、CLI、Electron、UI 与 macOS 系统行为保持一致，不再把浏览器 WebUI 当主交互面。

## Workstream Overview

### Workstream A — Reuse the Shared Desktop-First Surface
把 Windows 分支上已经成型的共享 renderer/store/page 改造整理成可在 macOS 分支复用的基础层。

### Workstream B — macOS Privileged Flow
把 `launchd` helper 与 `osascript` 一次性授权整合成统一的 macOS 图形化连接路径。

### Workstream C — Route Cleanup and Session Correctness
围绕 `fix-mac-route-cleanup` 这个分支目标，把路由清理、route-ready 生命周期和异常恢复做成可证明正确的系统行为。

### Workstream D — Packaging, Docs, Verification
完成 macOS 桌面端的分发、文档和真实场景验证。

## Detailed Tasks

### Task M1: Import and freeze the shared desktop-first renderer contract

**Files:**
- Modify: `webui/src/App.vue`
- Modify: `webui/src/components/NavBar.vue`
- Modify: `webui/src/api/desktop.ts`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/types/ecnu-vpn.d.ts`
- Modify: `webui/desktop/preload/index.ts`
- Modify: `webui/desktop/main/index.ts`
- Read/Cherry-pick from: `../windows/webui/src/**`, `../windows/webui/desktop/**`

**Boundary:** 只复用跨平台共享 contract、state 与壳层，不把 Windows 专属 driver/runtime UI 一起直接搬过来。

**Why it matters:** macOS 分支如果从头单独再做一套 renderer contract，会同时制造重复实现和跨平台漂移。最划算的路径是先把共享层对齐。

- [ ] 从 Windows 分支筛出可共享的提交单元：store contract、错误归一化、desktop shell、Dashboard/Service/Settings 的通用结构。
- [ ] 删除或替换其中所有 Windows 专属字段与文案，例如 Wintun/TAP。
- [ ] 冻结 macOS 分支的统一状态字段与错误字段，保持与 Windows 的跨平台共性尽量一致。
- [ ] 让 renderer 侧只通过 store 和 typed API 消费状态，不直接调用 Electron IPC。

**Acceptance:**
- macOS 分支具备与 Windows 分支一致的 desktop-first 壳层与 store contract。
- renderer 中不存在 Windows-only 语义泄漏。

**Dependencies:** 无，是整个 macOS GUI 收敛的入口任务。

**Parallelism:** Lane A 独立完成；其他 lane 可以并行看 native 侧，但页面交互不能先定稿。

### Task M2: Add macOS direct/elevated parity for helper-less GUI usage

**Files:**
- Modify: `src/app_api.cpp`
- Modify: `src/vpn.cpp`
- Modify: `src/vpn.hpp`
- Modify: `webui/desktop/main/index.ts`
- Modify: `webui/desktop/preload/index.ts`
- Modify: `webui/src/types/ecnu-vpn.d.ts`
- Modify: `webui/src/api/desktop.ts`
- Modify: `webui/src/stores/vpn.ts`

**Boundary:** 只处理 helper 缺失时的临时提权和 structured error，不改 route cleanup 具体逻辑。

**Why it matters:** 对 macOS 来说，“全面 UI 使用”成立的前提是未装 helper 时也有图形化可走的临时路径，而不是退回终端安装说明。

- [ ] 明确 helper path 与 direct/elevated path 的 action 语义边界。
- [ ] 让 Electron main 在 `darwin` 上通过 `osascript ... with administrator privileges` 执行 direct action，而不是回退为普通 action。
- [ ] 对用户取消授权、脚本执行失败、runtime 缺失、配置缺失、helper 缺失建立 `error_type` 分类。
- [ ] 让 store 侧获得 `currentSessionMode = helper | direct | elevated | disconnected` 的可消费状态。

**Acceptance:**
- 未安装 helper 时，桌面端可以完成一次性连接和断开。
- 用户取消管理员授权时，UI 得到的是明确的 `elevation_denied` 而不是泛化异常。

**Dependencies:** 依赖 M1 的共享 contract 基线。

**Parallelism:** Lane A + Lane B 联合，完成后才能收口 Dashboard。

### Task M3: Rebuild Dashboard as the true macOS control surface

**Files:**
- Modify: `webui/src/pages/DashboardPage.vue`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/components/StatusBadge.vue`
- Modify: `webui/src/composables/useSSE.ts`

**Boundary:** 不新增新页面，不改现有路由表。

**Why it matters:** macOS 用户的核心路径必须在 Dashboard 里完成，否则任何“全面 UI 使用”的说法都只是页面堆砌。

- [ ] 定义 macOS Dashboard 的状态机：helper ready、helper missing、elevated connecting、direct connected、helper connected、cleanup pending、runtime missing、authorization denied。
- [ ] 在 helper 缺失时，把“安装服务（推荐）”作为主动作，“仅本次连接”作为次动作。
- [ ] direct/elevated 会话要明确标示为临时模式，并提示安装 launchd helper 作为长期方案。
- [ ] 任何需要 route cleanup 完成的断开过程，都要在 UI 上有明确状态，而不是瞬时跳回 disconnected。

**Acceptance:**
- 新用户仅靠 Dashboard 就能理解“推荐安装 helper”与“本次授权连接”的区别。
- direct/elevated 模式不会被误描述成持久连接。

**Dependencies:** 依赖 M1、M2。

**Parallelism:** Lane B 主做；与 M4 可并行准备文案。

### Task M4: Rework ServicePage and SettingsPage for macOS-first messaging

**Files:**
- Modify: `webui/src/pages/ServicePage.vue`
- Modify: `webui/src/pages/SettingsPage.vue`
- Modify: `webui/src/stores/config.ts`
- Modify: `README.md`
- Modify: `README_CN.md`

**Boundary:** 保留 browser WebUI 配置作为兼容项，不删除底层字段。

**Why it matters:** macOS 产品路径的推荐叙事必须围绕 launchd helper、桌面端和一次性授权，而不是 sudo 命令或浏览器入口。

- [ ] ServicePage 要围绕 launchd helper 的安装状态、运行状态、推荐路径和临时路径展开。
- [ ] SettingsPage 要把浏览器兼容设置移入高级项，并把 runtime/connection 设置置于默认主视图。
- [ ] 文案中统一把 browser WebUI 定义为 compatibility/debug path，而不是常规入口。
- [ ] 所有 CLI 命令都移到二级说明或 `<details>`，不成为主内容块。

**Acceptance:**
- 页面主叙事始终是“桌面端优先 + 安装 helper 推荐 + 一次性授权兜底”。
- 用户不需要理解 sudo/terminal 也能完成主路径。

**Dependencies:** M1 先完成共享页面结构，M3 给出 Dashboard 术语后再统一文案。

**Parallelism:** 与 M3 高度耦合，可由同一前端 agent 负责。

### Task M5: Demote browser WebUI in CLI semantics and docs on macOS

**Files:**
- Modify: `src/main.cpp`
- Modify: `src/config.hpp`
- Modify: `src/config.cpp`
- Modify: `docs/user_guide.md`
- Modify: `README.md`
- Modify: `README_CN.md`
- Modify: `webui/README.md`

**Boundary:** 不删除 WebUI server，只改变默认行为和对外说明。

**Why it matters:** 如果 CLI 默认和文档仍然告诉用户“WebUI 自动启动”，那么 macOS 分支就没有真正完成 desktop-first 转向。

- [ ] 把 CLI 默认语义改成“连接并返回”，而不是“连接并带浏览器 WebUI 生命周期”。
- [ ] 如保留 `--webui`，则将其文案完全定义为兼容模式。
- [ ] 将文档中的“启动 VPN（含 WebUI）”全面替换为 desktop-first 叙事。
- [ ] 把 `webui_enabled` 默认值与帮助文本改到与桌面优先一致。

**Acceptance:**
- CLI、README、页面文案、设置默认值与真实行为一致。
- macOS 用户不会再从文档中被引导到浏览器作为主入口。

**Dependencies:** 可与 M3/M4 并行，但最终 wording 需等页面术语稳定。

**Parallelism:** Lane C 独立。

### Task M6: Make route cleanup a first-class macOS release criterion

**Files:**
- Modify: `src/helper.cpp`
- Modify: `src/tunnel.cpp`
- Modify: `src/helper_daemon_mac.cpp`
- Modify: `src/app_api.cpp`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/pages/DashboardPage.vue`
- Modify: `docs/user_guide.md`

**Boundary:** 不额外做 Linux/Windows 的共用抽象；本任务只解决 macOS 路由清理与状态可观测性。

**Why it matters:** `fix-mac-route-cleanup` 这个分支名本身就说明，macOS 的关键差距不仅是 UI，还有连接/断开/异常路径上的路由残留风险。只要这个点不闭环，GUI 再漂亮也不能算能全面使用。

- [ ] 梳理 route cleanup 的触发点：helper stop、direct stop、异常断开、helper uninstall、app 退出后 reconnect fail。
- [ ] 确认 `route-ready`、pid 文件、supervisor 状态和 UI status 的关系，避免 UI 显示 disconnected 但系统路由未清理。
- [ ] 若 cleanup 是异步的，UI 必须能展示 `cleanup pending` 或等价状态，而不是误导用户认为已完成。
- [ ] 为路由清理失败建立独立错误分类和恢复建议。

**Acceptance:**
- 所有断开与失败路径都能稳定清理临时路由与 runtime state。
- UI 能区分“已断开”与“正在清理/清理失败”。

**Dependencies:** M2 给出 direct/elevated contract 后才能完整验证。

**Parallelism:** Lane B + Lane C 共同推进，不能完全与 M2 脱钩。

### Task M7: Close the macOS packaging and distribution path

**Files:**
- Modify: `webui/package.json`
- Modify: `scripts/stage-openconnect-runtime-mac.sh`
- Modify: `runtime/README.md`
- Modify: `webui/build-resources/**`（若目录缺失则创建）
- Modify: `README.md`
- Modify: `README_CN.md`

**Boundary:** 本阶段做到“开发分发可用 + DMG 可构建”；签名、公证若证书不可得，可先写为下一阶段 gate，但必须明确记录。

**Why it matters:** macOS 上如果只有 `desktop:dev` 可用、没有稳定的 DMG / runtime bundle / helper install path，就仍然不属于“全面 UI 使用”。

- [ ] 明确 native binary、openconnect runtime、脚本与 Electron app bundle 的打包位置与查找路径。
- [ ] 把 `stage-openconnect-runtime-mac.sh` 与 Electron build 产物对齐，确保桌面端不依赖外部浏览器/命令行补资源。
- [ ] 如果 `webui/build-resources` 尚缺 macOS 图标或 DMG 配置资源，补齐最小可打包集。
- [ ] 明确是否支持 unsigned dev DMG、signed release DMG 两种路径，并在文档中区分。

**Acceptance:**
- macOS 分支可构建桌面包，并包含运行所需 native/runtime 资源。
- 打包路径与 README 描述一致。

**Dependencies:** 可与 M3-M6 并行，但最终验证依赖其完成。

**Parallelism:** Lane D 独立。

### Task M8: Execute the macOS full-UI verification matrix

**Files:**
- Modify: `docs/user_guide.md`
- Create or modify: `docs/superpowers/plans/verification-notes/*.md`（若团队需要留痕）
- Read: `build*`, `webui/release/`, `runtime/README.md`

**Boundary:** 不在这个任务里再新增功能，只记录证据与剩余缺口。

**Why it matters:** macOS 分支的风险不在“没有任何实现”，而在“系统层问题容易被 UI 表面覆盖”。验证矩阵必须围绕系统行为，而不是只看页面能不能打开。

- [ ] Native build: `cmake --build <mac build dir> --target exv`。
- [ ] Frontend build: `cd webui && npm run build && npm run build:electron`。
- [ ] Desktop dev path: `npm run desktop:dev` 连到 native 正常。
- [ ] Helper-installed path: 安装 launchd helper 后，连接/断开/重启桌面端都正常。
- [ ] Helper-missing elevated path: 未安装 helper 时，Dashboard 能触发一次性授权连接。
- [ ] Authorization denied path: 用户取消 `osascript` 授权后，UI 给出 `elevation_denied` 与恢复动作。
- [ ] Route cleanup path: direct disconnect、helper disconnect、异常连接失败、helper uninstall 后都不残留校园路由。
- [ ] Runtime missing path: 缺 openconnect 时 UI 提示准确。
- [ ] Docs parity: CLI、README、页面文案都不再暗示浏览器是主入口。

**Acceptance:**
- 每个场景都有明确 PASS/FAIL 与复现证据。
- route cleanup 被纳入与连接成功同等级的放行标准。

**Dependencies:** M1-M7 完成后执行。

**Parallelism:** 由独立验证 lane 执行，不与实现 lane 混合结论。

## Dependency Graph

- M1 是整个分支的共享基线，阻塞 M2、M3、M4。
- M2 阻塞 M3 与 M6，因为 Dashboard 和 route cleanup 的 UI/状态闭环都要依赖 direct/elevated contract。
- M3 与 M4 构成用户主流程页面，二者任一未完成都不能宣称 GUI-only。
- M5 可与 M3/M4 并行，但必须在 M8 前完成，否则对外叙事仍然错误。
- M6 是 macOS 分支的核心系统正确性任务，不能被降级成“后面再看”的清理项。
- M7 可并行推进，但打包验证要等 M3-M6 完成。
- M8 是最后闸门。

## Multi-Agent Split

### Lane A — Shared Renderer Porting
负责 M1，把 Windows 分支可共享的 UI/store/Electron contract 变成 macOS 分支的基线。

### Lane B — macOS Privileged UX
负责 M2、M3、M4，把 `launchd` helper 和 `osascript` 授权变成桌面用户流。

### Lane C — CLI / Route Cleanup / System Semantics
负责 M5、M6，收敛 CLI 默认行为、helper 生命周期和 route cleanup 正确性。

### Lane D — Packaging / Distribution
负责 M7，补齐 DMG/runtime/build-resource 路径。

### Lane E — Verification
负责 M8，以系统场景矩阵为准进行独立验收。

## Recommended Execution Order

1. 从 Windows 分支挑出共享变更，完成 M1。
2. Lane B 基于 M1 完成 M2。
3. 在 contract 稳定后，推进 M3 + M4。
4. Lane C 并行推进 M5 + M6，其中 M6 是 macOS 分支专属优先项。
5. Lane D 完成 M7。
6. Lane E 执行 M8，给出 macOS GUI-only readiness 结论。

## Cross-Branch Collaboration Strategy

macOS 分支不应重新发明下面这些内容，而应尽量从 `windows` 分支提取共享提交：

- `webui/src/App.vue` 与 `webui/src/components/NavBar.vue` 的桌面壳层。
- `webui/src/stores/vpn.ts` 的 contract、错误归一化与派生状态机制。
- `webui/src/api/desktop.ts`、`webui/src/types/ecnu-vpn.d.ts`、`webui/desktop/preload/index.ts` 的 typed desktop API。
- `DashboardPage.vue` / `ServicePage.vue` / `SettingsPage.vue` 的 service-first 结构。
- `main.cpp` 中 browser WebUI compatibility 化的跨平台部分。

macOS 分支需要保留并扩展自己的专属层：

- `launchd` helper 生命周期。
- `osascript` 管理员授权行为。
- Unix socket + `getpeereid` 的安全边界。
- route cleanup / route-ready / cleanup pending 的系统正确性。
- DMG 分发与 macOS runtime bundle。

## Release Exit Criteria

只有当下面 5 条同时满足，macOS 才能说接近“全面 UI 使用”：

1. 用户仅靠桌面端即可完成常规连接与配置。
2. helper 已安装与未安装两条路径都能在 GUI 中走通。
3. 用户取消授权、runtime 缺失、helper 缺失都有结构化错误与恢复动作。
4. route cleanup 在所有断开与失败路径下都被证明正确。
5. CLI、页面、文档、打包产物都共同指向 desktop-first，而不是 browser-first。