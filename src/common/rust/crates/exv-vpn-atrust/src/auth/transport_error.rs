//! 仅供认证传输和其 Underlay 原捕获点生成有界原因；不保存原错误对象。
use std::error::Error as _;

use super::AuthHttpError;

pub(crate) fn io_detail(error: &std::io::Error) -> String {
    if let Some(code) = error.raw_os_error() {
        bounded(&std::io::Error::from_raw_os_error(code).to_string())
    } else if let Some(tls) = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustls::Error>())
    {
        tls_detail(tls)
    } else {
        text_detail(&error.to_string())
    }
}

pub(crate) fn tls_detail(error: &rustls::Error) -> String {
    let detail = match error {
        rustls::Error::AlertReceived(alert) => format!("rustls: AlertReceived({alert:?})"),
        rustls::Error::DecryptError => "rustls: DecryptError".into(),
        rustls::Error::EncryptError => "rustls: EncryptError".into(),
        rustls::Error::InvalidMessage(reason) => format!("rustls: InvalidMessage({reason:?})"),
        rustls::Error::PeerMisbehaved(_) => "rustls: PeerMisbehaved".into(),
        rustls::Error::PeerIncompatible(_) => "rustls: PeerIncompatible".into(),
        rustls::Error::InvalidCertificate(_) => "rustls: InvalidCertificate".into(),
        rustls::Error::InappropriateMessage { .. } => "rustls: InappropriateMessage".into(),
        rustls::Error::InappropriateHandshakeMessage { .. } => {
            "rustls: InappropriateHandshakeMessage".into()
        }
        rustls::Error::PeerSentOversizedRecord => "rustls: PeerSentOversizedRecord".into(),
        rustls::Error::HandshakeNotComplete => "rustls: HandshakeNotComplete".into(),
        rustls::Error::General(reason) => format!("rustls: General: {}", text_detail(reason)),
        _ => "rustls: other error (detail redacted)".into(),
    };
    bounded(&detail)
}

pub(super) fn http_failure(operation: &'static str, error: &hyper::Error) -> AuthHttpError {
    // Hyper Display 在当前锁定依赖中仅给类型说明；不格式化其 Debug/request/response。
    let class = if error.is_parse() {
        "parse"
    } else if error.is_incomplete_message() {
        "incomplete message"
    } else if error.is_closed() {
        "closed"
    } else if error.is_canceled() {
        "canceled"
    } else if error.is_timeout() {
        "timeout"
    } else if error.is_body_write_aborted() {
        "body write aborted"
    } else if error.is_user() {
        "user"
    } else {
        "transport"
    };
    let description = format!("hyper {class}: {error}");
    let mut detail = text_detail(&description);
    let mut source = error.source();
    while let Some(cause) = source {
        if let Some(io) = cause.downcast_ref::<std::io::Error>() {
            return AuthHttpError::Http {
                operation,
                io_kind: Some(io.kind()),
                raw_os_error: io.raw_os_error(),
                // io_detail 同时检查 get_ref 中的 typed TLS，不假设它是 source 节点。
                detail: bounded(&format!("{description}; {}", io_detail(io))),
            };
        }
        if let Some(tls) = cause.downcast_ref::<rustls::Error>() {
            detail = bounded(&format!("{description}; {}", tls_detail(tls)));
            break;
        }
        detail = bounded(&format!(
            "{description}; {}",
            text_detail(&cause.to_string())
        ));
        source = cause.source();
    }
    AuthHttpError::Http {
        operation,
        io_kind: None,
        raw_os_error: None,
        detail,
    }
}

pub(crate) fn text_detail(text: &str) -> String {
    let text: String = text.chars().filter(|ch| !ch.is_control()).collect();
    let lower = text.to_ascii_lowercase();
    let sensitive_start = [
        "cookie",
        "authorization",
        "password",
        "passwd",
        "credential",
        "token",
        "secret",
        "bearer",
        "username",
        "webvpn",
        "sid",
        "ticket",
        "signkey",
        "sign_key",
        "certificate",
        "public key",
        "private key",
    ]
    .iter()
    .filter_map(|marker| lower.find(marker))
    .chain(text.find('='))
    .min();
    let prefix = &text[..sensitive_start.unwrap_or(text.len())];
    let mut result = prefix
        .split_whitespace()
        .map(|word| {
            if word.contains("://") {
                "[url redacted]"
            } else {
                word
            }
        })
        .collect::<Vec<_>>()
        .join(" ");
    if sensitive_start.is_some() {
        result.push_str(" [sensitive detail redacted]");
    }
    bounded(&result)
}

fn bounded(text: &str) -> String {
    let mut result = String::new();
    for ch in text.chars().filter(|ch| !ch.is_control()) {
        if result.len() + ch.len_utf8() > 256 {
            break;
        }
        result.push(ch);
    }
    result
}
