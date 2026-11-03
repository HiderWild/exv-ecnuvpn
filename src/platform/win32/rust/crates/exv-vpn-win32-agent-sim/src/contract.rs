//! aTrust 本机 agent 端点契约（纯函数，可离线单测）。
//!
//! 来源：`docs/research/atrust/2026-10-05-l3-local-agent-simulation-spec.md` §2；
//! 兜底纪律：《计划》§四 S2（杜绝"形似成功"吞票据）。
//! 旧门户走 HTTP POST（统一信封、无 Content-Type），新门户在 HTTP `/v1/detect` 判定后
//! 走 WebSocket 状态通道。本模块只负责「路径/消息 → 响应 JSON」的**静态**部分，不含
//! I/O；`/v1/service/reportEnvBeforeLogin` 与 `/v1/service/start` 属**动态**端点，由
//! [`crate::consume`]（真实网关核销）在 [`crate::server`] 分发，不经本模块。
//!
//! 关键判据（门户侧）：
//! * 旧门户 HTTP：`code === 0`（`SFERRCODE.OK`）。
//! * 新门户 WS：按 `sdpTraceId` 匹配回调；响应 `mode="request-response"`、`type="response"`
//!   且回显 `sdpTraceId`；心跳 `mode="heartbeat"`、`type="pong"`。
//!
//! 兜底纪律（S2）：未知 `/v1/service/*` 一律显式错误（[`AGENT_INNER_ERROR_CODE`]），
//! 不再盲目成功。门户选路依据（auth.js `Fi`：`g.aio?Promise.reject(ea):Vi()`）：我们
//! 的官方 UA 含 `aTrustTray-Windows-Plat`，使 `g.client`/`g.aio` 均为真，agent 端点
//! 错误以**可见失败**上抛，不会切到浏览器路径 `Vi()` 改变选路——证据锚定（官方
//! bundle 字节偏移直引，2026-10-07 复核）见
//! `docs/research/atrust/2026-10-05-atrust-device-identity-and-signature-spec.md`
//! §附二「门户 agent 路径选择证据锚定」；良性命名空间
//! （`/v1/aUem/*`、`/v1/base/*`、`/v1/update/*`）与 `/v1/service/checkMITMAttack`
//! 是 L3 规格表列出的探测/遥测类端点，维持成功兜底。

use serde_json::{Value, json};

/// WebSocket 信封常量（与门户 `detect_client` 内常量逐字一致）。
pub const WS_MODE_REQUEST_RESPONSE: &str = "request-response";
pub const WS_TYPE_REQUEST: &str = "request";
pub const WS_TYPE_RESPONSE: &str = "response";
pub const WS_MODE_HEARTBEAT: &str = "heartbeat";
pub const WS_TYPE_PING: &str = "ping";
pub const WS_TYPE_PONG: &str = "pong";
/// 本机 agent 侧 `from` 标识（门户侧为 `SDPBrowserClient`）。
pub const WS_FROM_AGENT: &str = "SDPAgent";

/// 未实现/内部失败端点的官方业务码：门户 `SFERRCODE.TRUSTCORE_BASIC_ERROR = 11119000`
/// （`aTrustTray\resources\app\uem_tool\assets\utils.js` 错误码枚举直读；门户把该码映射为
/// "Client inner request error" 并以可见错误上抛）。未在官方侧观察到"未知端点"专用码，
/// 选用该通用 agent 内部错误码。
pub const AGENT_INNER_ERROR_CODE: i64 = 11119000;

/// 去掉查询串，只保留路径。
#[must_use]
pub fn path_only(path: &str) -> &str {
    path.split(['?', '#']).next().unwrap_or(path)
}

/// 未实现端点的显式错误体（含未实现的 `/v1/service/*`）。
#[must_use]
pub fn unimplemented_body() -> Value {
    json!({
        "code": AGENT_INNER_ERROR_CODE,
        "message": "unimplemented agent endpoint",
        "data": {},
    })
}

/// 良性命名空间兜底成功（L3 规格表：aUem/base/update 是探测/遥测类，不承载票据）。
fn benign_namespace_ok(path: &str) -> bool {
    path.starts_with("/v1/aUem/")
        || path.starts_with("/v1/base/")
        || path.starts_with("/v1/update/")
}

/// 旧门户 HTTP 端点静态响应（`POST`，成功判据 `code === 0`）。
///
/// 动态端点（`/v1/service/reportEnvBeforeLogin`、`/v1/service/start`）由
/// [`crate::consume`] 处理，不在此列；它们若意外落到本函数，同样落入显式错误
/// （fail-closed，不会退化为成功）。`/v1/service/checkMITMAttack` 在 L3 规格表中是
/// 成功兜底端点（ECNU `enable=0` 时门户跳过该调用；signKey 由 reportEnvBeforeLogin
/// 请求自带材料派生，见 [`crate::consume`]），保持成功。
#[must_use]
pub fn http_body(path: &str, gateway: &str) -> Value {
    let path = path_only(path);
    match path {
        "/v1/detect" => json!({"code": 0, "data": {}}),
        "/v1/service/init" => json!({
            "code": 0,
            "data": {
                "status": "online",
                "addr": gateway,
                "sidTicket": "",
                "capacities": {},
            },
            "security": {},
        }),
        "/v1/service/status" => json!({
            "code": 0,
            "data": {
                "status": "online",
                "addr": gateway,
                "tunnelStatus": "tunnelAvailable",
            },
        }),
        "/v1/service/queryAppModuleInfo" => {
            json!({"code": 0, "resourceReady": true, "data": {}})
        }
        "/v1/service/events" => json!({"code": 0, "data": {"id": 0, "events": []}}),
        "/v1/aUem/statusV2" => json!({"uemStatus": 0}),
        "/v1/service/checkMITMAttack" => json!({"code": 0, "data": {}}),
        _ if benign_namespace_ok(path) => json!({"code": 0, "data": {}}),
        _ => unimplemented_body(),
    }
}

/// 新门户 WebSocket 响应 `data` 字段（门户回调直接消费 `response.data`）。
///
/// 与 HTTP 层级不同（门户 `detect_client` 逐字证据）：
/// * `/v1/service/status` 的 `status` 在顶层、`addr`/`tunnelStatus` 在 `data` 内；
/// * `/v1/service/events` 订阅响应为 `{events,id,pid}`。
///
/// 兜底纪律与 [`http_body`] 一致：未知 `/v1/service/*` 显式错误（票据流当前走 HTTP，
/// 但若门户未来把核销端点挪到 WS 通道，同样不得吞掉）。
#[must_use]
pub fn ws_data(path: &str, gateway: &str) -> Value {
    let path = path_only(path);
    match path {
        "/v1/service/status" => json!({
            "status": "online",
            "data": {"addr": gateway, "tunnelStatus": "tunnelAvailable"},
        }),
        "/v1/service/queryAppModuleInfo" => {
            json!({"code": 0, "resourceReady": true, "data": {}})
        }
        "/v1/aUem/statusV2" => json!({"uemStatus": 0}),
        "/v1/service/events" => json!({"events": [], "id": 0, "pid": 0}),
        "/v1/service/checkMITMAttack" => json!({"code": 0, "data": {}}),
        _ if benign_namespace_ok(path) => json!({"code": 0, "data": {}}),
        _ => unimplemented_body(),
    }
}

/// 把请求信封中的 `sdpTraceId`/`seq`/`version` 原样回显到响应（存在才回显）。
fn echo(reply: &mut serde_json::Map<String, Value>, request: &Value, key: &str) {
    if let Some(value) = request.get(key) {
        reply.insert(key.to_owned(), value.clone());
    }
}

/// 处理一条 WebSocket 文本消息，返回需要回送的文本（`None` 表示不回）。
///
/// 覆盖：裸 `ping` → `pong`；心跳信封 → `mode="heartbeat"/type="pong"`；
/// 请求信封 → `mode="request-response"/type="response"` 并回显 `sdpTraceId`。
#[must_use]
pub fn ws_reply(text: &str, gateway: &str) -> Option<String> {
    let trimmed = text.trim();
    if trimmed == WS_TYPE_PING {
        return Some(json!(WS_TYPE_PONG).to_string());
    }
    let request: Value = serde_json::from_str(trimmed).ok()?;
    let mode = request.get("mode").and_then(Value::as_str).unwrap_or_default();
    let kind = request.get("type").and_then(Value::as_str).unwrap_or_default();

    // 心跳：门户发送 `{mode:"heartbeat",type:"ping",...}`，须以 pong 回并以 sdpTraceId 匹配回调。
    // 信封侧判据只看 `type=ping`（heartbeat 信封与裸 ping 串同形；mode 不参与判据）。
    if kind == WS_TYPE_PING {
        let mut reply = serde_json::Map::new();
        reply.insert("mode".to_owned(), json!(WS_MODE_HEARTBEAT));
        reply.insert("type".to_owned(), json!(WS_TYPE_PONG));
        reply.insert(
            "path".to_owned(),
            request.get("path").cloned().unwrap_or_else(|| json!("ping")),
        );
        echo(&mut reply, &request, "sdpTraceId");
        echo(&mut reply, &request, "seq");
        echo(&mut reply, &request, "version");
        reply.insert("from".to_owned(), json!(WS_FROM_AGENT));
        reply.insert("data".to_owned(), json!({}));
        return Some(Value::Object(reply).to_string());
    }

    if kind == WS_TYPE_REQUEST || mode == WS_MODE_REQUEST_RESPONSE {
        let path = request
            .get("path")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let mut reply = serde_json::Map::new();
        reply.insert("mode".to_owned(), json!(WS_MODE_REQUEST_RESPONSE));
        reply.insert("type".to_owned(), json!(WS_TYPE_RESPONSE));
        reply.insert("path".to_owned(), json!(path));
        echo(&mut reply, &request, "sdpTraceId");
        echo(&mut reply, &request, "seq");
        echo(&mut reply, &request, "version");
        reply.insert("from".to_owned(), json!(WS_FROM_AGENT));
        reply.insert("data".to_owned(), ws_data(path, gateway));
        return Some(Value::Object(reply).to_string());
    }
    None
}
