# Windows Desktop-First Full-UI Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 在 `windows` 分支上，把当前已经成型的 Windows 桌面端能力收束成真正可发布、可验证、可完全通过 GUI 使用的产品闭环，让用户在未接触终端的前提下完成安装服务、一次性提权连接、驱动准备、运行时诊断、连接与断开。

**Architecture:** 保持 `exv desktop-rpc` 作为唯一 native 控制面，helper / direct / elevated 三种执行路径统一收敛为结构化状态与错误协议；Electron main 负责 UAC 与高权限执行；renderer 只消费 Pinia store 的派生状态，不自行做平台分支；安装包与便携版都携带一致的 native runtime 资源。

**Tech Stack:** C++17, CMake, Electron 39, Vue 3, Pinia, Vue Router, Vite, PowerShell `Start-Process -Verb RunAs`, Windows Service, Wintun/TAP runtime packaging, NSIS + portable build.

---

## Branch Baseline

当前 `windows` 工作树已经不是从零开始，而是“已做完主干、尚未完成收口”的状态：

- `src/app_api.cpp` 已经引入 `status.get.direct`、`vpn.connect.direct`、`vpn.disconnect.direct`，并让 `vpn.connect` / `vpn.disconnect` 在 helper 不可用时可切到 direct path。
- `webui/desktop/main/index.ts` 已经有 `ecnu-vpn:vpn-command`，并在 Windows 侧通过 UAC 提权后执行 direct-mode action。
- `webui/src/stores/vpn.ts` 已经有 `VpnErrorType`、`normalizeError()`、`serviceInstalled`、`recommendedConnectMode`、`currentSessionMode` 等派生状态。
- `webui/src/pages/DashboardPage.vue`、`ServicePage.vue`、`SettingsPage.vue` 已经出现 service-first 文案与浏览器兼容降级。
- `src/main.cpp` 已经增加 `--webui` 并把默认 CLI 语义改为“连接并返回”。
- `README.md` / `README_CN.md` 已经部分转向 desktop-first，且 Windows runtime staging 与 package pipeline 已经存在。

因此，这个分支的规划重点不是再做基础设施，而是把当前多处“已存在但还不够收敛”的实现做成可交付系统。

## Implementation Goal

达到下面这个可验收状态：

1. Windows 用户首次打开桌面端即可完成配置与连接，不需要浏览器 WebUI，也不需要终端。
2. 未安装服务时，GUI 能明确推荐安装服务，并可在用户选择时触发一次性 UAC 提权完成临时连接。
3. runtime 缺失、Wintun/TAP 缺失、服务缺失、用户拒绝 UAC 等错误，都会被 UI 准确区分并给出下一步动作。
4. NSIS 安装版与 portable 版都携带一致的 native 运行时资源，用户路径一致。
5. CLI、文档、打包脚本和 UI 叙事保持一致，不再把浏览器 WebUI 当默认主入口。

## Workstream Overview

### Workstream A — Contract Convergence
统一 native / Electron / renderer 三层的状态与错误协议，避免 helper / direct / elevated 三套语义并行漂移。

### Workstream B — Zero-Terminal User Flow
把 Dashboard、Service、Settings、Runtime/Driver 提示真正串成“不会用终端也能完成”的 GUI 用户旅程。

### Workstream C — Packaging and Runtime Integrity
让 portable / installer / staged runtime / embedded assets 的组合稳定、可验证、可复现。

### Workstream D — Release Gate and Proof
把这条链路从“代码存在”提升到“场景验证通过”，以真实 Windows 使用矩阵作为放行条件。

## Detailed Tasks

### Task W1: Freeze the Windows privileged-operation contract

**Files:**
- Modify: `src/app_api.cpp`
- Modify: `src/vpn.cpp`
- Modify: `src/vpn.hpp`
- Modify: `webui/desktop/main/index.ts`
- Modify: `webui/desktop/preload/index.ts`
- Modify: `webui/src/types/ecnu-vpn.d.ts`
- Modify: `webui/src/api/desktop.ts`
- Modify: `webui/src/stores/vpn.ts`

**Boundary:** 只收敛 contract 和 payload，不改页面布局与文案。

**Why it matters:** 现在 Windows 分支已经同时存在 helper action、direct action、elevated bridge 和 store 层错误归一化；如果 contract 不冻结，后续页面、验证和文档都会建立在不稳定接口上。

- [ ] 定义统一的成功返回字段：`connected`、`mode`、`network_ready`、`interface`、`internal_ip`、`pid`、`supervisor_pid`、`server`、`username`。
- [ ] 定义统一的失败返回字段：`ok: false`、`error_type`、`message`、`recoverable`、`recommended_action`。
- [ ] 约定 direct action 与 helper action 的语义边界：`status.get` 代表“推荐状态查询”，`*.direct` 代表“跳过 helper 的显式动作”。
- [ ] 让 Electron main 对 UAC 拒绝、native stderr、JSON parse failure、unknown action 都输出统一错误对象，而不是部分抛异常、部分返回对象。
- [ ] 清理 renderer 中所有对错误 message 的字符串推断，统一由 `error_type` 与派生状态驱动。

**Acceptance:**
- 所有连接/断开/状态查询路径都能回到同一套 `VpnStatus | VpnError` 协议。
- renderer 代码不再依赖错误字符串判断下一步操作。
- `helper`、`direct`、`elevated` 三种路径在 UI 中都能标出 `mode`。

**Dependencies:** 无，是整个 Windows 收尾阶段的基座。

**Parallelism:** 可以由 Lane A 独立完成；在这一步冻结前，Lane B 只允许做静态文案与布局，不允许最终定稿交互逻辑。

### Task W2: Close the Dashboard state machine for real GUI-only use

**Files:**
- Modify: `webui/src/pages/DashboardPage.vue`
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/composables/useSSE.ts`
- Read: `webui/src/components/StatusBadge.vue`

**Boundary:** 不改路由结构，不引入新页面。

**Why it matters:** 当前 Dashboard 已经有 service-first 雏形，但仍缺少真正的状态机闭环，例如空状态、加载状态、安装服务后刷新状态、direct session 断开路径、UAC 拒绝后的可恢复操作提示。

- [ ] 把 Dashboard 拆成明确状态：`service-ready disconnected`、`service-missing disconnected`、`elevated connecting`、`direct connected`、`helper connected`、`error recoverable`、`error blocking`。
- [ ] 给每个状态定义唯一主 CTA 和唯一次 CTA，避免同屏出现互相冲突的动作。
- [ ] direct/elevated 连接成功后，明确展示临时模式告警与推荐升级动作。
- [ ] runtime missing、driver missing、service missing、elevation denied 四类错误都要在 Dashboard 上呈现不同的下一步动作。
- [ ] 让首次进入页面时同时拉取 `status` 与 `serviceStatus`，且在安装服务、断开、连接完成后能稳定刷新。

**Acceptance:**
- 新用户从 Dashboard 即可完成“安装服务”或“仅本次连接”两条路径。
- direct/elevated 模式下不会再出现“请确保 VPN 服务正在运行”这类错误提示误导。
- UAC 拒绝后用户能在页面内理解失败原因并继续操作，而不是只看到一条泛化错误。

**Dependencies:** 依赖 W1。

**Parallelism:** Lane B 主做；可与 W3 并行推进视觉与文案，但交互逻辑需等 W1 后收口。

### Task W3: Finish the service-first support pages

**Files:**
- Modify: `webui/src/pages/ServicePage.vue`
- Modify: `webui/src/pages/SettingsPage.vue`
- Modify: `webui/src/stores/config.ts`
- Modify: `webui/src/stores/vpn.ts`

**Boundary:** 保留现有信息架构，不新增导航项。

**Why it matters:** “全面 UI 使用”不只看 Dashboard。用户一旦进入 Service / Settings，就必须看到自洽的推荐路径、兼容路径和驱动/runtime 状态，而不是回到命令行语境。

- [ ] ServicePage 要完整区分：服务已安装且运行、已安装未运行、未安装、桌面端不可管理 四种状态。
- [ ] ServicePage 的“仅本次连接”说明要明确标为兜底方案，不与推荐路径争主入口。
- [ ] SettingsPage 中 runtime 与 driver 区块要补齐缺失时的动作建议，例如准备 Wintun、安装 TAP、切换 runtime source。
- [ ] 浏览器兼容设置继续保留，但只以 `<details>` 形式作为高级项出现，不进入默认操作流。
- [ ] Windows 专属的 Wintun/TAP 选择、状态展示、安装动作在 UI 中要闭环到“下一次连接会如何生效”。

**Acceptance:**
- ServicePage 和 SettingsPage 在不依赖终端说明的情况下可完成服务和驱动/runtime 管理。
- 页面默认叙事清晰表达“桌面端 + 服务安装优先，浏览器兼容仅为高级项”。

**Dependencies:** W1 可先行，W2 最好先定义状态机后再统一文案。

**Parallelism:** 与 W2 高度相关，可由同一前端 agent 处理，或拆成 Service/Settings 两个独立 agent，但需要共享 store contract。

### Task W4: Remove the last CLI/WebUI semantic drift on Windows

**Files:**
- Modify: `src/main.cpp`
- Modify: `src/config.hpp`
- Modify: `src/config.cpp`
- Modify: `src/app_api.cpp`
- Modify: `README.md`
- Modify: `README_CN.md`
- Modify: `docs/user_guide.md`

**Boundary:** 不删除 browser WebUI 兼容能力，只改变默认语义与对外叙事。

**Why it matters:** 只要 CLI 默认、配置默认、帮助文本和文档还在讲“WebUI 自动启动”，那就还没有真正完成 desktop-first。

- [ ] 把 `webui_enabled` 默认值、`main.cpp` help、运行时提示、README 用语完全对齐到“CLI 默认只连接并返回，WebUI 需要显式 `--webui`”。
- [ ] 清理 Windows 用户路径中任何把浏览器 WebUI 说成主入口的文案。
- [ ] 确保 direct-mode / elevated mode 的说明进入帮助文本与用户文档，而不是隐藏在实现细节里。
- [ ] 保留兼容入口，但让所有说明都把其定义为调试/兼容路径。

**Acceptance:**
- CLI、配置默认值、页面文案、README 与行为一致。
- Windows 用户不会再从文档里得到“默认走浏览器”的错误预期。

**Dependencies:** 可与 W2/W3 并行，但最终 wording 需要参考 W2 完成后的页面术语。

**Parallelism:** 可由 Lane C 独立推进。

### Task W5: Turn driver/runtime readiness into a first-class Windows workflow

**Files:**
- Modify: `src/app_api.cpp`
- Modify: `src/config.cpp`
- Modify: `webui/src/pages/SettingsPage.vue`
- Modify: `webui/src/stores/config.ts`
- Modify: `scripts/stage-openconnect-runtime-win.ps1`
- Modify: `runtime/README.md`

**Boundary:** 只解决 Windows runtime/driver 可用性与用户动作，不做 Linux/macOS 共用抽象。

**Why it matters:** Windows 上“全面 UI 使用”的最大真实门槛不是按钮，而是 openconnect runtime、Wintun/TAP、内置资源和安装版/便携版是否真正可用。

- [ ] 在 runtime status 和 driver status 返回里补齐“当前缺什么、下一步怎么补”的字段，而不只是裸状态。
- [ ] UI 对 `runtime_missing`、`wintun_missing`、`tap_missing` 的动作建议分别收敛到明确 CTA。
- [ ] stage 脚本与 runtime README 要准确描述 required DLL、wintun.dll 和可选 TAP 资源。
- [ ] 安装驱动后的 UI 反馈要说明“是否立即生效 / 是否需重连 / 是否需重新打开应用”。

**Acceptance:**
- 纯 GUI 用户能判断当前缺的是 runtime、Wintun 还是 TAP，并在 UI 或安装器支持下完成补齐。
- 便携版与安装版都不会因 runtime/driver 资产缺失而进入“只会报错、不知道怎么办”的状态。

**Dependencies:** 依赖 W1 的错误类型冻结；与 W3 并行。

**Parallelism:** Lane C 可与 Lane B 并行。

### Task W6: Close Windows packaging and install-path parity

**Files:**
- Modify: `webui/package.json`
- Modify: `webui/scripts/prepare-native.cjs`
- Modify: `webui/build-resources/**`（若当前目录仍缺失则需要创建）
- Modify: `scripts/stage-openconnect-runtime-win.ps1`
- Modify: `runtime/README.md`
- Modify: `README.md`
- Modify: `README_CN.md`

**Boundary:** 只针对 Windows 发布物，不扩展到 macOS notarization。

**Why it matters:** 只要安装版和便携版携带的 runtime、helper service 注册行为、驱动资产和文档不一致，就不能说“Windows GUI 可全面使用”。

- [ ] 明确安装版与便携版各自是否自动注册服务、何时触发 UAC、缺少服务时如何回退。
- [ ] 检查 `electron-builder` 配置、`prepare-native.cjs`、installer include 文件和 runtime staging 之间的输入输出契约。
- [ ] 如果 `webui/build-resources` 仍缺关键资源，要把图标、installer include、版本资源、许可说明补齐到可打包状态。
- [ ] 定义 portable 与 NSIS 的最小验证差异：首次启动、是否触发安装服务、runtime 查找路径、卸载行为。

**Acceptance:**
- 两种发布物都能完成首次启动与连接主路径。
- 不存在“开发模式可用、打包后缺文件/缺脚本/缺资源”的断层。

**Dependencies:** 可与 W2-W5 并行，但最终验证必须在 W7 之后做。

**Parallelism:** Lane D 独立。

### Task W7: Add observability and recovery signals that support a supportless UI

**Files:**
- Modify: `webui/src/stores/vpn.ts`
- Modify: `webui/src/pages/DashboardPage.vue`
- Modify: `webui/src/pages/LogsPage.vue`
- Modify: `src/app_api.cpp`
- Modify: `webui/desktop/main/index.ts`

**Boundary:** 不引入后端遥测系统；只做本地可见的状态与日志信号。

**Why it matters:** 真正的 GUI-only 产品不能要求用户看 stderr 或手动跑命令调试；UI 必须自己给出足够的恢复信息。

- [ ] 把 last error、current session mode、service state change、driver/runtime warnings 整合到统一的用户可见反馈机制。
- [ ] LogsPage 需要能从连接失败上下文跳转或快速定位近期关键日志。
- [ ] Dashboard 需要明确展示“当前是 helper/direct/elevated 哪种会话”。
- [ ] 安装服务、安装驱动、UAC 拒绝、direct connect 失败后要有稳定 toast / inline status / retry affordance。

**Acceptance:**
- 用户在 UI 内就能理解最近一次失败属于哪一类、下一步该点什么。
- 支持人员复盘时不需要先要求用户打开命令行。

**Dependencies:** 依赖 W1-W3。

**Parallelism:** 与 W6 并行。

### Task W8: Execute the Windows release verification matrix

**Files:**
- Modify: `docs/user_guide.md`
- Create or modify: `docs/superpowers/plans/verification-notes/*.md`（若团队需要记录证据）
- Read: `build/`, `webui/release/`, `runtime/README.md`

**Boundary:** 本任务只做验证与记录，不再继续扩展范围。

**Why it matters:** 这个分支目前最大的风险不是“没代码”，而是“代码存在但没有用真实 Windows 用户路径证明可行”。

- [ ] Native build: `cmake --build build --target exv`。
- [ ] Frontend build: `cd webui && npm run build && npm run build:electron`。
- [ ] Desktop dev: `npm run desktop:dev` 能正常连到 native。
- [ ] Service-installed path: 安装服务后，连接/断开/重启应用/再次进入状态页都正确。
- [ ] No-service elevated path: 未安装服务时，Dashboard 可触发一次性连接，断开后能回到可恢复状态。
- [ ] UAC denial path: 用户拒绝授权时得到 `elevation_denied`，UI 不崩溃，仍能继续安装服务或重试。
- [ ] Runtime missing path: 缺少 `openconnect.exe` 或 DLL 时，UI 给出准确提示。
- [ ] Driver missing path: Wintun / TAP 缺失时，Settings 能显示并引导下一步。
- [ ] Portable vs installer parity: 两种构建路径都完成一次完整连接闭环。

**Acceptance:**
- 每个场景都有明确 PASS/FAIL 与证据。
- 若仍有 FAIL，必须被收敛成具体 bug 列表，而不是模糊表述“还有点问题”。

**Dependencies:** W1-W7 完成后执行。

**Parallelism:** 由独立验证 agent 执行，不能和实现 agent 混做最终结论。

## Dependency Graph

- W1 是硬前置，阻塞 W2、W3、W5、W7 的最终收口。
- W2 与 W3 共同构成“页面主流程闭环”，任一未完成都不能宣称 GUI-only。
- W4 可与 W2/W3 并行，但必须在 W8 前完成，否则文档/行为仍会冲突。
- W5 与 W6 可并行推进，前者解决 Windows 专属 runtime/driver 体验，后者解决发布物一致性。
- W7 依赖 W1-W3，但可在 W5/W6 期间并行做本地反馈机制。
- W8 是阶段闸门，必须由独立验证 lane 执行。

## Multi-Agent Split

### Lane A — Native/Electron Contract
负责 W1，必要时参与 W5 的 native 字段补充。

### Lane B — Renderer/User Flow
负责 W2、W3、W7，处理 Dashboard / Service / Settings / Logs 与 store 协调。

### Lane C — CLI / Docs / Runtime Guidance
负责 W4、W5，收敛 `main.cpp`、配置默认值、README 与 runtime staging 说明。

### Lane D — Packaging / Release Engineering
负责 W6，并在 W8 中与验证 lane 配合提供安装版/便携版产物。

### Lane E — Verification
只负责 W8，不参与前述实现，以免“自己改自己验”。

## Recommended Execution Order

1. Lane A 完成 W1，并冻结 payload/错误类型。
2. Lane B 基于 W1 完成 W2 + W3。
3. Lane C 并行推进 W4 + W5。
4. Lane D 在 Lane C 提供 runtime/staging 约束后完成 W6。
5. Lane B 收口 W7。
6. Lane E 执行 W8，形成最终 Windows GUI 放行结论。

## Cherry-Pick Guidance for the macOS Branch

Windows 分支中以下提交单元应尽量做成可 cherry-pick 的共享变更，因为 macOS 分支可以直接复用：

- store / api / types 的 contract 与错误归一化。
- `App.vue` / `NavBar.vue` / `DashboardPage.vue` / `ServicePage.vue` / `SettingsPage.vue` 的通用 desktop-first 壳层与页面结构。
- `main.cpp` 的 CLI 默认语义调整与 WebUI compatibility 文案。
- README / README_CN / `docs/user_guide.md` 中与 desktop-first、build order 相关的跨平台共性改动。

Windows 专属项如 Wintun/TAP、NSIS、runtime staging、UAC 细节不应直接硬拷到 macOS 分支。

## Release Exit Criteria

只有当下面 5 条同时满足，Windows 才算真正接近“全面 UI 使用”：

1. 新用户首次使用只靠桌面端即可完成连接。
2. 未安装服务时，UI 能完整处理一次性提权连接与失败恢复。
3. runtime / driver / service / UAC 错误都能分类并给出操作建议。
4. 安装版与便携版的运行时资产和行为一致。
5. CLI、UI、文档、打包链路都使用同一套产品叙事。