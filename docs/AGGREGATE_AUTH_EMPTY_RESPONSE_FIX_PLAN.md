# Aggregate-Auth Empty Response — 修复计划

> **状态**：待执行修复计划（非调试发现）。
> **范围**：用户报告 “正确密码也连不上、错误信息显示为 `auth_failed: aggregate auth response is empty`” 的根因修复，以及由此暴露的协议读层、错误归类、握手可观测性、split-handshake 交互回调四处缺口。
> **关联文档**：
> - 早期调试计划 `docs/superpowers/plans/2026-06-19-aggregate-auth-empty-response-debug-plan.md` 由本文件取代——后者把多个“代码事实”当成了“已证实根因”，本计划修正这些判断。
> - 验证报告 `docs/superpowers/reports/2026-06-19-core-rpc-lane-isolation-and-connect-pipeline-concurrency-verification.md` 明确记录“真实网关连接”未自动化覆盖。
> **执行守则**：
> - 不先改 helper / service。无服务、有服务同错；aggregate-auth 不经 helper。
> - 不先改 RPC lane / WebView shell / 密码读取逻辑。
> - 不提交原始响应二进制（含 cookie/opaque/token），只提交脱敏摘要。
> - 不重写请求 shape；当前请求已带 XML content-type、aggregate headers、identity encoding，需要 live summary 证据后再决定是否动 path/User-Agent/cookie。

---

## 1. 已证实的代码事实（保留）

| # | 结论 | 证据 |
|---|---|---|
| F1 | `aggregate auth response is empty` 字面量只在 `parse_aggregate_auth_response()` body trim 后为空时产生 | `src/vpn_engine/protocol/aggregate_auth.cpp:376` |
| F2 | 三个调用点都在 HTTP status 进入 `[200,300)` 之后才 parse body | `src/vpn_engine/protocol/production_transport.cpp:575` 起的三处 |
| F3 | 新并发连接路径在 `protocol_handshake` 分支内部调用 `NativeHandshakeJob::run()` 完成 aggregate-auth | `src/core/app_api/desktop_vpn_actions.cpp:442` |
| F4 | `start_from_handshake()` 仅 adopt，已认证完成的 session 不再重新 auth | `src/vpn_engine/native_engine.cpp:178` |
| F5 | Win32 生产依赖确实是真 TLS + `ProductionProtocolTransport`，没有 fake transport 注入 | `src/platform/win32/default_engine_deps.cpp:11` |
| F6 | 真实网关连接尚未被自动化覆盖 | `docs/superpowers/reports/2026-06-19-core-rpc-lane-isolation-and-connect-pipeline-concurrency-verification.md:122` |

## 2. 需要修正的判断（弃用）

| # | 旧表述 | 修正 |
|---|---|---|
| C1 | “服务端返回空 200 OK” | 仅证明 `read_http_response()` 交给 parser 的 `body` 字段为空。是“服务端真的回 0 字节”还是“客户端没读到 body”，从代码不可区分。 |
| C2 | “TCP/TLS 读写均成功” | 说过头。当前只能说写入成功且至少读到了 HTTP 头；不能保证 body 被完整读完。 |
| C3 | “TLS 读到 0 但当成 ok” | 不准确。`read_more()` 遇到 0 字节 chunk 会返回 `transport_closed`（`src/vpn_engine/protocol/production_transport.cpp:1019`）。真正缺口是“无 `Content-Length` 时不会继续读 body”。 |
| C4 | “提交 raw-init-response.bin 作为证据” | 风险高：真实认证响应可能含 cookie / opaque / token / username / 网关信息。仅提交脱敏摘要（status, content-type, content-length, transfer-encoding, body 字节数与首尾若干字节的哈希/长度）。 |

---

## 3. 必须修 #1 — HTTP body framing 不完整

**位置**：`src/vpn_engine/protocol/production_transport.cpp:1033`（`read_http_response()`）。

**当前行为**：
- 仅完整支持 `Content-Length`。
- 没有 `Content-Length` 时，把 buffer 里已经到达的字节当 body，然后清空 buffer。
- 不支持 `Transfer-Encoding: chunked`。
- 不支持无 `Content-Length` 且 `Connection: close` 的 “read until EOF” body 定界。
- `Content-Length: 0` 直接落到 XML parser，最终生成 `auth_response_invalid`。

**目标修复**：

1. 支持 `Transfer-Encoding: chunked`：按 chunk-size 行读取、解码、长度上限保护、丢弃 trailers。
2. 支持无 `Content-Length` 且 `Connection: close` 的响应：读到 EOF 视作 body 结束；正常 EOF（非异常断开）不应当成读失败。
3. `Content-Length: 0`（或 chunked 解出零字节）不再传给 XML parser；在三个 auth 调用点（init / submit-reply / followup）改成显式协议错误，错误码用 `auth_protocol_mismatch` 或 `auth_protocol_error`，错误消息含脱敏字段：`status` / `content-type` / `content-length` / `transfer-encoding` / `body_bytes`。
4. 调试日志只记录脱敏 summary，不写入 raw cookie / body / set-cookie。

**测试（先 RED 后 GREEN）**：在 `tests/native_production_transport_test.cpp` 加：
- `chunked_response_split_across_tls_reads_parses_aggregate_auth`：header 与 chunk body 分多次 TLS read，能解出 `<config-auth>`。
- `chunked_response_in_single_tls_read_does_not_treat_chunk_framing_as_xml`：同一次 TLS read，body 不混入 chunk-size 行。
- `connection_close_without_content_length_reads_body_until_eof`：body 在后续 read 才到达，最终能成功 parse。
- `content_length_zero_reports_protocol_error_not_auth_failed`：返回 `auth_protocol_*` 类错误，不返回 `auth_failed`，不暴露 password / cookie。

**通过标准**：上述 RED 先失败、修复后通过；现有 `native_production_transport_test`、`native_aggregate_auth_test` 继续通过；`no_secret_in_logs_test` 不退化。

---

## 4. 必须修 #2 — 错误归类不能把协议错误显示成密码错误

**位置**：
- `src/feedback/feedback.cpp:126`（`feedback::resolve_error_code()`）
- `src/core/tunnel_controller/core_error_mapper.cpp:243`（`CoreErrorMapper::from_native_error()`）
- `webui/src/stores/vpn.ts:257`（前端 normalize / presentation）

**当前行为**：raw `auth_response_invalid` 因为字符串包含 `auth` 被归为 `auth_failed`，前端显示“VPN 密码错误 / 认证失败”。即便协议层已经定位为响应畸形，用户也会被引导去重输密码——这是误导性错误的主要源头之一。

**目标修复**：
1. 最小改动：把 raw `auth_response_invalid`、`auth_response_too_large` 映射到已有 canonical 码 `auth_protocol_mismatch`。**不**新增前端错误类型。
2. `CoreErrorMapper::from_native_error()` 同步修正，避免 status / controller 路径上仍归到 auth/password 类。
3. 前端复用 `auth_protocol_mismatch` 现有展示路径——按钮应是“查看日志”，不是“重新输入密码”。

**测试**：
- `tests/feedback_test.cpp`：`resolve_error_code("auth_response_invalid", "...") == "auth_protocol_mismatch"`；`auth_response_too_large` 同。
- `tests/core_error_mapper_test.cpp`：`from_native_error("auth_response_invalid", ...)` 不产出密码重试类错误。
- Web host test（`webui/host/__tests__/`）：raw code `auth_response_invalid` 的 normalize / presentation 不走 `auth_failed` 分支，主操作按钮文案为“查看日志”等而非“重新输入密码”。

**通过标准**：上述测试新增并通过；现有归类测试不退化；用户输入正确密码时不再看到 “VPN 密码错误”。

---

## 5. 必须修 #3 — Prepared handshake 缺少 event sink

**位置**：
- `src/core/app_api/desktop_vpn_actions.cpp:459`（protocol_branch 构造 `NativeHandshakeJob` 时使用 `default_native_engine_dependencies()`，没有挂 `event_sink`）
- `src/vpn_engine/native_handshake_job.cpp:128`（job.run() 内部仍会发射 `auth.started` / `auth.failed` / `cstp.failed` 等事件）

**当前行为**：关键握手事件被丢弃。一旦 prepared-handshake 阶段失败，没有日志能告诉运维“失败发生在哪一步”，导致排障只能从 status 字段反推。

**目标修复**：
1. **不**新写一个 static sink 自己手搓日志、自己手搓脱敏。
2. 复用现有 `EngineEventBridge` 的日志/脱敏逻辑；如其内部状态绑定 runner 不便共享，则抽出一个共享的 “log-only engine event sink”，runner 与 prepared-handshake 都使用同一份。
3. 在 protocol_branch 创建本地 sink 实例并设置 `deps.event_sink = &sink`；sink 生命周期覆盖 `job.run()` 即可。

**测试**：
- `engine_event_bridge_test` 或新增 focused 测试：`auth.failed` 事件经 sink 进入 `LogFacade` 且字段已脱敏（无 password / cookie / token）。
- `app_api_status_contract_test` 加源代码 guard：protocol_handshake 分支必须设置 `deps.event_sink`，并使用共享 sink 类型而非匿名内联 lambda。
- `no_secret_in_logs_test` 继续通过。

**通过标准**：prepared-handshake 失败路径在日志里能看到与传统 runner 路径一致的事件序列；不引入 secret 泄漏。

---

## 6. 必须修 #4 — Split-handshake 缺失 auth interaction handler

**位置**：
- `src/core/app_api/desktop_vpn_actions.cpp:459`（protocol_branch 用 default deps，未注入 `auth_interaction_handler`）
- `src/core/tunnel_controller/core_session_runner.cpp:75`（旧 `start()` 里会设置 handler；新流程已把认证提前到 prepared-handshake）

**说明**：这不是本次 empty-body 现象的直接证据来源，但它是新并发握手路径的真实缺口。一旦网关要求选组（group select）或二次验证（challenge），prepared-handshake 阶段会直接以 `auth_group_required` / `auth_challenge_required` 失败，UI 没有机会继续——`vpn.authInteraction.get` 当前只查 runner，runner 此时还没有创建。

**目标修复**：
1. 抽出共享的 `AuthInteractionCoordinator`，由 connect job / controller 共同持有（生命周期与本次 connect 尝试一致）。
2. protocol_branch 的 deps 设置 `auth_interaction_handler = coordinator.handle(...)`；阻塞等待用户响应或取消令牌触发。
3. `vpn.authInteraction.get` / `vpn.authInteraction.respond` 查询同一个 coordinator，而不是只查已启动的 runner。
4. serial tail / adopt 完成后，仍保持现有 runner / controller 查询语义不变（adopted 之后由 runner 接管交互，coordinator 关闭）。

**测试**：
- 用 fake transport 在 prepared-handshake 阶段返回 group / challenge 响应。
- 断言 `vpn.authInteraction.get` 能看到 pending interaction，`respond` 后 handshake 在同一 connect 尝试内继续。
- 取消令牌触发时，coordinator 释放等待者并向上抛出 `user_cancelled`。

**通过标准**：prepared-handshake 路径与旧 direct-runner 路径都能完成 group/challenge continuation，且各自的 cancel 路径不出现 hang 或泄漏 handler。

---

## 7. 显式不做（防止 scope creep）

- ❌ 不优先改 helper / service。无服务和有服务同错，且 aggregate-auth 不走 helper。
- ❌ 不先重写 request shape（path / User-Agent / cookie / Accept-Language）。仅在 live 脱敏 summary 证明确实是真空 200 或入口错误时再考虑。
- ❌ 不提交 raw 响应二进制；只提交脱敏 observation（status, headers 白名单, body byte count, content sniff prefix length）。
- ❌ 不动 RPC lane / WebView shell / 密码读取路径。
- ❌ 不在前端新增错误类型；复用 `auth_protocol_mismatch`。

## 8. 执行顺序建议

修复彼此独立，可以并行评审，但合入顺序建议：

1. **#2 错误归类**（最小、最快用户可见改进，且为后续给出更准 telemetry。
2. **#3 event sink**（落 telemetry 钩子，使 #1 的真实修复有日志可观察）。
3. **#1 HTTP framing**（核心修复；落地后用真实网关复测）。
4. **#4 auth interaction**（功能补齐，依赖 #3 的 sink 抽出）。

每一步先 RED 再 GREEN，提交里写明对应章节编号。

---

## 9. 验收标准

### 9.1 自动化

以下测试套件全部通过：
- `native_production_transport_test`
- `native_aggregate_auth_test`
- `feedback_test`
- `core_error_mapper_test`
- `engine_event_bridge_test`
- `core_session_runner_test`
- `vpn_actions_test`
- `app_api_status_contract_test`
- `no_secret_in_logs_test`
- `pnpm --dir webui test:host`
- `vue-tsc -b`（webui 类型检查）

新增的 RED 测试在修复前必须先失败，修复后通过。

### 9.2 手动真实网关

在有/无 helper service 两种模式下分别验证：

| 输入 | 期望 |
|---|---|
| 正确密码 | 不再失败为 `auth_failed: aggregate auth response is empty`；连接成功，或失败时给出 `auth_protocol_mismatch` 类码 + 脱敏摘要。 |
| 错误密码 | 错误码为 `auth_rejected` 或明确的认证拒绝码，前端按钮提示重输密码。 |
| 网关返回畸形/空响应 | 错误码为 `auth_protocol_mismatch`，日志含脱敏 `status` / 选定 header / `body_bytes`，前端按钮提示“查看日志”。 |
| 网关要求 group select / challenge | UI 收到 pending auth interaction 并能继续完成握手。 |

### 9.3 安全自检

- 日志、错误消息、telemetry 中均无 password、cookie、Set-Cookie、authorization header、opaque、token、session-id。
- 自动化中的脱敏断言（`no_secret_in_logs_test` 与 web host policy 测试）覆盖新增日志通路。

---

## 10. 文档输出

修复完成后追加（不替换本文件）：
- `docs/AGGREGATE_AUTH_EMPTY_RESPONSE_FIX_REPORT.md` — 简短验收报告：实际定位结论（H1/H2/H3 中哪一项被证实）、脱敏 summary、自动化与手动验证日志摘要。
- 必要时更新 `docs/HELPER_PROTOCOL.md` / `docs/DESKTOP_RPC_CONTRACT_V2.md` 中错误码章节，补 `auth_protocol_mismatch` 与 raw `auth_response_invalid` / `auth_response_too_large` 的映射表。

旧 `docs/superpowers/plans/2026-06-19-aggregate-auth-empty-response-debug-plan.md` 不强制删除，但应在其首部加入 `> Superseded by docs/AGGREGATE_AUTH_EMPTY_RESPONSE_FIX_PLAN.md` 的指向，避免后续 agent 误把其中的“已证实”表述当事实使用。
