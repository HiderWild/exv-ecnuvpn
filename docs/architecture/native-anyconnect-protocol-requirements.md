# Native AnyConnect 协议深度对齐需求（方案 B / v2）

> **定位**：这是原生引擎（`src/vpn_engine/`）从“可用的 v1 净室原型”升级为
> “长期、深度、与 OpenConnect 行为对齐的 AnyConnect 兼容客户端”的需求文档。
>
> **裁决关系**：
> - 本文 **取代** `native-anyconnect-clean-room-spec.md` 第 1 节中“Unsupported in v1”的范围限制；
>   该 v1 文档仍作为净室纪律与历史范围的参考保留。
> - 与 `10-requirements.md` / `00-constitution.md` 的组件边界一致，不引入新的边界违例。
> - 与 `docs/architecture/new_start_point.md` 的最高指示不冲突。
>
> **目标（用户原话）**：“目的不是快速可用，而是从长远角度出发，直接面向长期而深度重构”，
> 并“确保设置中之前所有与 OpenConnect 相关的功能，在原生引擎中也都能实现”
> （路由表解析、客户端标识伪装传出等）。
>
> **净室纪律（强制）**：本文只描述 **线上行为 / wire 行为 / 设置语义**。
> 实现时 **禁止** 转写 `reference/openconnect-upstream/` 的源代码、解析器结构、状态机结构、
> 常量表或注释。OpenConnect 仅作为“行为参照点”。本文引用 upstream 文件:行号
> 仅用于 **行为核对**，不得照抄实现。

---

## 0. 现状诊断（为什么必须做方案 B）

通过对 `src/vpn_engine/protocol/*` 与 `reference/openconnect-upstream/*` 的深入比对，
确认原生引擎当前实现的是 **clientless WebVPN（HTML 门户）流程**，
而 ECNU 服务器期望的是 **AnyConnect XML aggregate-auth 流程**。这是连接在
第一个 HTTP 读处即 `transport_closed` 的根因。

| 项 | 当前原生实现 | 真实 AnyConnect / OpenConnect 期望 | 证据 |
|----|--------------|-----------------------------------|------|
| 登录路径 | `GET /+CSCOE+/logon.html`（clientless 门户） | `POST /`，body 为 `<config-auth client="vpn" type="init">` | `production_transport.cpp:20`；upstream `auth.c` |
| 认证表示 | HTML `<form>` 解析 | XML `<config-auth>` / `<auth>` / `<session-token>` | `auth.cpp:321,405,457` |
| 会话 cookie | `webvpn_session=` | `webvpn=`（来自 `<session-token>`） | `auth.cpp:457` |
| CSTP 路径 | `/CSCOT/` | `CONNECT /CSCOSSLC/tunnel` | `production_transport.cpp:21` |
| 必备请求头 | 仅 User-Agent/Accept/Connection/Cookie/X-CSTP-Version | 还需 `X-Transcend-Version: 1`、`X-Aggregate-Auth: 1`、`Accept-Encoding: identity` | `production_transport.cpp:171-220` |
| 服务器配置解析 | 仅 Address/Netmask/MTU/Split-Include/Bypass-Route | 还需 DNS/NBNS/Default-Domain/Banner/Split-Exclude/Keepalive/DPD/Rekey/Lease/IPv6/Tunnel-All-DNS | `cstp.cpp:278` |
| DTLS | **完全不存在**（仅有 `disable_dtls` 设置位） | 旧式 Master-Secret 或现代 X-DTLS12-CipherSuite/PSK | `config.hpp:18`；无 UDP 传输实现 |
| `extra_args` | UI 可填、被存储，但 **原生引擎完全忽略** | OpenConnect CLI 透传语义 | `app_api.cpp:699`；`engine.hpp` 无对应字段 |
| 客户端标识伪装 | useragent 默认已伪装 AnyConnect，但 **CSTP CONNECT 用的是硬编码 `ECNU-VPN Native`** | 所有请求统一使用配置的 AnyConnect User-Agent | `production_transport.cpp:22` |

**结论**：v1 的整套认证与隧道协商需要按 AnyConnect 协议重写，并补齐 DTLS 与全部
服务器下发字段的解析与应用。

---

## 1. 范围

### 1.1 v2 必须支持（In scope）

- AnyConnect **XML aggregate-auth** 用户名/密码认证状态机。
- group-select、二次密码 / challenge / tokencode（2FA）交互。
- CSTP over TCP/TLS 主隧道（`CONNECT /CSCOSSLC/tunnel`）。
- 服务器下发的 **完整** 网络配置解析与应用：IPv4 地址/掩码/MTU、DNS、NBNS、
  Default-Domain、Banner、Split-Include、Split-Exclude、Tunnel-All-DNS、IPv6。
- DPD / keepalive / rekey 定时器。
- DTLS 加速（旧式 + 现代），失败时回退 CSTP-only。
- 断线重连与 cookie 复用、session/idle timeout 处理。
- 客户端标识（User-Agent）在 **所有** 请求上统一伪装。
- 设置项 `extra_args` 的明确语义（见 §7）。
- CSD / host-scan 探测的识别与（可选）应答（见 §3.4）。

### 1.2 v2 暂不支持（Out of scope，但需优雅降级）

- SAML / 浏览器登录。
- 证书注册（enrollment）。
- GlobalProtect / Pulse / Juniper / Fortinet 等非 AnyConnect 协议。

遇到上述情况时，必须给出 **明确的、可上报的错误码**（见 §8），不得静默挂死。

---

## 2. 认证：XML aggregate-auth 状态机

> 行为参照：upstream `auth.c`（`<config-auth>` / `<auth>` / `<session-token>`）。
> **禁止转写其解析代码或状态机结构。**

### R-AUTH-1 init 请求

- 必须以 `POST /` 发送，body 为 XML `<config-auth client="vpn" type="init">`，
  包含 `<version who="vpn">`、`<device-id>`、`<group-access>`（server URL）等字段。
- 请求头必须包含：`X-Transcend-Version: 1`、`X-Aggregate-Auth: 1`、
  `Accept-Encoding: identity`、配置的 AnyConnect `User-Agent`、
  `Content-Type: application/xml; charset=utf-8`。
- 响应解析必须接受网关返回的 `text/xml`（例如 `text/xml; charset=utf-8`）
  XML `<config-auth>`。

验收：抓包显示 init POST body 为合法 XML；服务器返回 `<config-auth>` 而非 HTML 门户。

### R-AUTH-2 auth-reply 解析

- 必须解析服务器返回的 `<config-auth ... type="...">`：
  - `<auth id="...">`：`main` / `success` / `error` 状态。
  - `<form>` 内 `<input>`：构造下一步 `auth-reply` 的字段集合（用户名、密码、
    `group_list`、二次凭据等）。
  - `<opaque>`：必须 **原样回显**（echo）到后续请求。
  - `<session-token>` / `<session-id>`：成功时提取，映射为 `webvpn=` cookie。
  - `<error>`：映射到结构化错误码。

验收：单元测试覆盖 init→main→success 三段式样例。

### R-AUTH-3 group-select

- 当 auth-reply 含 `group_list` 时，必须能按配置（默认/用户选择）选择 group，
  并在 `auth-reply` 中回传所选 group。

### R-AUTH-4 2FA / challenge

- 必须支持二次密码、challenge 文本提示、tokencode 输入。
- 这些交互必须通过事件总线向上层 UI 暴露（事件类型见 §8），并能回填用户输入。

### R-AUTH-5 会话 cookie

- 成功后将 `<session-token>` 映射为 `webvpn=<token>` cookie，用于 CSTP CONNECT 与重连。

---

## 3. CSTP 隧道协商

### R-CSTP-1 CONNECT 请求

- 必须发送 `CONNECT /CSCOSSLC/tunnel HTTP/1.1`，并携带：
  - `Cookie: webvpn=<token>`
  - 配置的 AnyConnect `User-Agent`（**不得** 再用硬编码 `ECNU-VPN Native`）
  - `X-CSTP-Version: 1`、`X-CSTP-Hostname`、`X-CSTP-Address-Type: IPv6,IPv4`
  - `X-CSTP-Base-MTU`、`X-CSTP-MTU`（来自配置 MTU）
  - `X-CSTP-Accept-Encoding`（按是否支持压缩）
  - `X-Transcend-Version: 1`、`X-Aggregate-Auth: 1`
  - DTLS 相关请求头（见 §5）

### R-CSTP-2 服务器配置头完整解析

必须解析并应用以下 `X-CSTP-*` / `X-DTLS-*` 响应头（缺失字段按默认处理）：

| 头 | 含义 | 应用位置 |
|----|------|----------|
| `X-CSTP-Address` / `X-CSTP-Netmask` | IPv4 地址/掩码 | `TunnelMetadata.internal_ip4_address/netmask` |
| `X-CSTP-Address-IP6` / `X-CSTP-Netmask-IP6` | IPv6 地址/前缀 | **新增** IPv6 字段 |
| `X-CSTP-MTU` / `X-DTLS-MTU` | 隧道 MTU | `TunnelMetadata.mtu` |
| `X-CSTP-DNS` | DNS 服务器（可多次） | **新增** DNS 应用 |
| `X-CSTP-NBNS` | WINS 服务器 | **新增** |
| `X-CSTP-Default-Domain` | 默认搜索域 | **新增** |
| `X-CSTP-Split-Include` | 分流包含路由 | `TunnelMetadata.routes` |
| `X-CSTP-Split-Exclude` | 分流排除路由 | **新增** exclude 路由 |
| `X-CSTP-Banner` | 登录横幅（URL 编码） | **新增**，向 UI 上报 |
| `X-CSTP-Keepalive` | keepalive 周期 | **新增** 定时器 |
| `X-CSTP-DPD` | dead-peer-detection 周期 | **新增** 定时器 |
| `X-CSTP-Rekey-Time` / `X-CSTP-Rekey-Method` | 密钥轮换 | **新增** |
| `X-CSTP-Lease-Duration` / `X-CSTP-Idle-Timeout` / `X-CSTP-Session-Timeout` | 会话生命周期 | **新增** |
| `X-CSTP-Tunnel-All-DNS` | 是否全量 DNS 走隧道 | **新增**，影响 DNS 应用策略 |
| `X-CSTP-Disconnected-Timeout` | 断线容忍 | **新增** |

验收：对 `reference/openconnect-upstream/tests/fake-cisco-server.py` 的响应做解析，
所有上述字段被正确提取并体现在 `TunnelMetadata`（需扩展，见 §6）。

### R-CSTP-3 STF 帧

- 必须实现 STF 帧（magic `0x53 0x54 0x46 0x01`，be16 length，type byte，1 字节保留）。
- 包类型至少支持：`0=DATA`、`3=DPD_OUT`、`4=DPD_RESP`、`5=DISCONNECT`、
  `7=KEEPALIVE`、`8=COMPRESSED`、`9=TERMINATE`。
- 必须正确响应 DPD 请求（回 `DPD_RESP`）并按周期发起本端 keepalive/DPD。

### R-CSTP-4 read 错误语义

- 0 字节读不得一律映射为 `transport_closed`；必须区分：
  正常 server-disconnect、空闲超时、鉴权拒绝（HTTP 状态码）与真正的传输中断，
  分别映射不同错误码（见 §8）。

### R-CSTP-5 压缩

- 协商 `X-CSTP-Content-Encoding`（如 `lzs` / `lz4`）时按服务器要求处理；
  ECNU 默认 `Accept-Encoding: identity`，但解析逻辑必须存在以避免误判。

---

## 3.4 CSD / host-scan（可选）

### R-CSD-1

- 当 auth 阶段返回 `<host-scan>`（ticket / token / URIs / `sdesktop=` cookie）时，
  必须能识别。
- v2 至少实现“跳过/最小应答”路径以通过 ECNU 服务器（若 ECNU 启用了 host-scan）；
  无法满足时给出明确错误码，不得挂死。

---

## 4. DPD / keepalive / rekey

### R-TIMER-1

- 按 R-CSTP-2 解析出的周期运行：keepalive、DPD（含超时判定 → 触发重连）、rekey。
- rekey 到期：按 `X-CSTP-Rekey-Method`（`new-tunnel` / `ssl`）执行密钥/隧道轮换。

---

## 5. DTLS 加速

> 行为参照：upstream `dtls.c` / `dtls12`。**禁止转写实现。**

### R-DTLS-1 旧式（legacy）

- 解析 `X-DTLS-Session-ID`、`X-DTLS-Master-Secret`（48 字节）、`X-DTLS-CipherSuite`、
  `X-DTLS-Port`，建立 DTLS over UDP 通道。

### R-DTLS-2 现代

- 支持 `X-DTLS12-CipherSuite` + PSK（EXPORTER label `"EXPORTER-openconnect-psk"`）方式。

### R-DTLS-3 MTU 与回退

- 按 DTLS 头部开销正确计算 DTLS MTU。
- DTLS 状态机：`NOSECRET → ... → ESTABLISHED`；任一阶段失败必须 **回退 CSTP-only**，
  不得使整体连接失败。
- 当 `disable_dtls=true`（设置项）时，完全跳过 DTLS，仅用 CSTP。

验收：`disable_dtls` 设置真正生效；DTLS 协商失败时连接仍能在 CSTP 上工作。

---

## 6. `TunnelMetadata` 结构扩展（与 §3 对应）

当前 `src/vpn_engine/session_state.hpp:27` 仅有
`interface_name/index/ip4_address/ip4_netmask/mtu/routes/server_bypass_ips`。
必须扩展为（字段名以实现为准，语义如下）：

- `ip6_address` / `ip6_prefix`
- `dns_servers: vector<string>`
- `nbns_servers: vector<string>`
- `default_domain: string`
- `search_domains: vector<string>`
- `split_include_routes` / `split_exclude_routes`（区分 include / exclude）
- `tunnel_all_dns: bool`
- `banner: string`
- `keepalive_seconds` / `dpd_seconds` / `rekey_seconds` / `rekey_method`
- `lease_duration` / `idle_timeout` / `session_timeout` / `disconnected_timeout`

JSON schema（`tunnel_metadata_to_json`）必须同步扩展，并保持向后兼容（旧字段不改名）。

---

## 7. 设置功能对齐（用户明确要求）

| 设置项 | 当前状态 | v2 要求 |
|--------|----------|---------|
| `useragent`（客户端标识伪装） | 默认已伪装，但 CSTP CONNECT 用硬编码 | **所有** 请求（init/auth/CONNECT）统一使用配置的 User-Agent |
| `routes`（路由表解析/应用） | 默认 9 条 ECNU CIDR + Split-Include | 与服务器 Split-Include/Exclude 合并应用；区分 include/exclude |
| DNS 推送 | 无 | 解析 `X-CSTP-DNS` 并由平台层应用（win/darwin/linux），受 `Tunnel-All-DNS` 影响 |
| `mtu` | 透传到 CONNECT | 与 `X-CSTP-MTU`/DTLS MTU 协商取值 |
| `disable_dtls` | 仅存储，无实现 | 真正控制 DTLS 协商（见 §5） |
| `extra_args` | 存储但被忽略 | **明确语义**：v2 支持一组 **白名单** 透传项（如 `--no-dtls`、`--useragent`、`--csd-wrapper` 对应的内部开关映射）。不在白名单内的参数必须显式忽略并向 UI 上报“未支持”，不得静默假装生效。 |
| `auto_reconnect` | 已有 | 与 §9 重连/cookie 复用对齐 |

> `extra_args` 决策：原生引擎不是 CLI，因此 **不** 直接执行任意 OpenConnect 参数。
> v2 将常用参数映射为内部能力开关（白名单），其余明确降级。该决策必须在 UI 文案中体现。

---

## 8. 错误码与事件

复用既有 feedback 错误码体系（`src/feedback/`），并新增/明确以下分类：

- `auth_protocol_mismatch`：服务器返回 HTML 门户而非 XML（早失败可诊断）。
- `auth_challenge_required` / `auth_group_required`：需要 2FA / group 选择（非错误，触发 UI 交互事件）。
- `auth_rejected`：凭据/challenge 被拒。
- `csd_required_unsupported`：host-scan 要求但无法满足。
- `dtls_unavailable`（信息级，回退 CSTP，不致命）。
- `tunnel_disconnected` vs `transport_closed`：区分服务器主动断开与传输异常。
- `session_timeout` / `idle_timeout`：区分超时类断开（影响是否自动重连）。

每个引擎事件必须经 `LoggingEventSink` 镜像到 logger（已存在），并持久化失败码到
`native-session-state.json`（已存在 `persist_native_session_failure`）。

---

## 9. 重连与会话生命周期

### R-RECONN-1

- 保存 `webvpn=` cookie，重连时优先复用 cookie（免重新输密码），失败再回退完整认证。
- `session_timeout` 到期：终止并要求重新认证（不自动用旧 cookie）。
- `idle_timeout` / DPD 失败 / DTLS 抖动：在 `auto_reconnect` 下触发重连，遵守退避策略。

---

## 10. 测试与验收策略

> 净室：测试以 **wire 行为** 为准，不依赖 upstream 源结构。

- **解析单元测试**：每个 `X-CSTP-*` / `X-DTLS-*` 头、`<config-auth>` 各形态、STF 帧
  编解码、DPD/keepalive 帧，逐项单测。
- **认证状态机测试**：init→main→success、challenge、group-select、error 各路径。
- **一致性夹具**：以 `reference/openconnect-upstream/tests/fake-cisco-server.py`
  作为 **黑盒** 一致性服务器（仅观察其 HTTP/CSTP 响应行为，不读其内部实现），
  跑端到端握手与配置下发解析。
- **回退测试**：DTLS 协商失败 → CSTP-only 仍连通；`disable_dtls=true` 生效。
- **错误映射测试**：HTML 门户响应 → `auth_protocol_mismatch`；0 字节读的多种成因分别映射。
- **现有门禁**：Windows `ctest --preset windows-release`、macOS `ctest --preset macos-release`
  必须保持全绿；新功能不得回归既有 26 项。
- **真机验收**（需 ECNU 网络 + 凭据 + 管理员，人工执行）：完整连接、DNS/路由生效、
  断线重连、2FA（若启用）。

---

## 11. 分阶段实施路线（建议）

| 阶段 | 内容 | 退出标准 |
|------|------|----------|
| B0 | `TunnelMetadata` + JSON schema 扩展（§6） | 编译通过 + schema 单测 |
| B1 | XML aggregate-auth init/auth-reply/session-token（§2，单密码） | 对 fake-cisco-server 成功认证 |
| B2 | CSTP CONNECT `/CSCOSSLC/tunnel` + 完整头解析（§3） | 隧道地址/路由/DNS 正确下发 |
| B3 | DNS/路由（include/exclude）平台层应用（§7） | win/darwin 真机 DNS+路由生效 |
| B4 | DPD/keepalive/rekey 定时器（§4） | 长连接稳定，rekey 不掉线 |
| B5 | DTLS（旧式+现代）+ CSTP 回退（§5） | DTLS 通，且失败优雅回退 |
| B6 | 2FA/group-select/challenge（§2 R-AUTH-3/4） | 多因子样例通过 |
| B7 | CSD/host-scan 识别 + 重连/cookie 复用（§3.4/§9） | host-scan 场景不挂死；重连免密 |
| B8 | `extra_args` 白名单映射 + 错误码/事件全量（§7/§8） | UI 真实反映支持/降级 |

每阶段独立可验收、独立 ctest 全绿后再进入下一阶段。

---

## 12. 关键文件索引（实现起点）

- 协议传输：`src/vpn_engine/protocol/production_transport.cpp`
- 认证：`src/vpn_engine/protocol/auth.cpp`
- CSTP 帧/头：`src/vpn_engine/protocol/cstp.cpp`
- 隧道元数据/状态：`src/vpn_engine/session_state.{hpp,cpp}`
- 引擎契约：`src/vpn_engine/engine.hpp`
- 平台路由/DNS 应用：`src/platform/win32/native_ip_config.cpp`、
  `src/platform/darwin/native_route_config.cpp`
- TLS 流（已稳定，复用）：`src/platform/{win32,darwin}/native_tls_stream.cpp`
- 配置：`src/config.hpp`、`src/platform/*/config_defaults*.cpp`
- 行为参照（**仅参照，禁止转写**）：`reference/openconnect-upstream/{auth,http,cstp,dtls,ssl,tun}.c`
