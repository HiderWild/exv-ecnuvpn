//! Darwin 认证失败细分；复用 native 脱敏数值槽，不改变 Common 认证语义。
//! Unknown namespace 表示宿主内部协议码，绝不冒充 POSIX errno。
use exv_vpn_wire::generated as wire;

const SESSION_INVALIDATED: i64 = 0x4558_5601;
const AUTH_REJECTED: i64 = 0x4558_5602;

#[must_use]
pub fn native(session_invalidated: bool) -> wire::RedactedNativeError {
    wire::RedactedNativeError {
        category: wire::NativeErrorCategory::Protocol as i32,
        namespace: wire::NativeErrorNamespace::Unknown as i32,
        code: if session_invalidated {
            SESSION_INVALIDATED
        } else {
            AUTH_REJECTED
        },
    }
}

#[must_use]
pub fn is_atrust_auth(error: &wire::VpnError) -> bool {
    error.code == wire::ErrorCode::Unauthorized as i32
        && error.stage == wire::ErrorStage::ConnectingControl as i32
        && error.native.as_ref().is_some_and(|n| {
            n.category == wire::NativeErrorCategory::Protocol as i32
                && n.namespace == wire::NativeErrorNamespace::Unknown as i32
                && matches!(n.code, SESSION_INVALIDATED | AUTH_REJECTED)
        })
}

#[must_use]
pub fn is_session_invalidated(error: &wire::VpnError) -> bool {
    is_atrust_auth(error)
        && error
            .native
            .as_ref()
            .is_some_and(|n| n.code == SESSION_INVALIDATED)
}
