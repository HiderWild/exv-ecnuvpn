//! aTrust 设备签名派生与请求签名（纯函数，无 I/O）。
//!
//! 依据 `docs/research/atrust/2026-10-05-atrust-device-identity-and-signature-spec.md`
//! 实验A（A.1–A.7，2026-10-07 定稿）：signKey 只由网关 authConfig 下发的公开握手材料
//! （`devicePubKeyMod`/`devicePubKeyExp`/`challenge`）加客户端硬编码 pepper 派生，
//! 无本地私有材料、无 RSA 私钥运算。`X-Request-Sig`/`x-sdp-signature` 是
//! HMAC-SHA256(signKey 原始 32 字节, uri[||body]) 的大写十六进制，不含 nonce/时间戳
//! （A.4：1363 条同输入签名恒定）。派生结果按 crate 脱敏约定属敏感材料
//! （`error.rs` 规则 1：由公钥材料派生的片段），持有方负责用 [`zeroize::Zeroizing`] 包裹。
//!
//! 摘要/HMAC 复用既有依赖 `ring`，不引入新第三方 crate（规格 A.1 提及的 `sha2`
//! 与 `ring::digest` 同为标准 SHA-256，字节结果一致）。

use ring::{digest, hmac};
use zeroize::Zeroizing;

/// 派生 pepper，规格 A.2 规范形态（64 字符 ASCII）。官方二进制内以 16 字符块乱序存储
/// （`SdpPcMITMAdapter.dll` @0x8dad0 等四处），取用函数重排回本形态；实现直接硬编码
/// 规范形态。该常量是客户端程序内嵌的混淆常量，不是凭据。
const PEPPER: &[u8] = b"3uW5IEy8KwDaOMK8uw1TmNr50U3aK1Qdu8b6vopXxGstzan3AJXxVNR6piuKi5Nq";

/// 按 A.1 公式从握手材料派生 signKey（32 字节原始值）：
///
/// ```text
/// t  = devicePubKeyMod || devicePubKeyExp          // ASCII 拼接
/// E  = SHA256(t || PEPPER)                         // 32 字节原始摘要
/// B  = UPPERCASE_HEX(E)                            // 64 字符大写十六进制
/// C  = B || challenge                              // challenge 取 base64 字符串原样，不解码
/// signKey = SHA256(C) XOR E                        // 逐字节，i=0..31
/// ```
///
/// 不参与派生的字段（A.1 [事实]）：csrfToken、ticket、encryptedChallenge、mitmSig、
/// guid、addr、enable、cert 摘要；`randomS` 恒等于 `challenge` 原文。
#[must_use]
pub fn derive_sign_key(
    device_pub_key_mod: &str,
    device_pub_key_exp: &str,
    challenge_b64: &str,
) -> [u8; 32] {
    // t || PEPPER 一次连拼后做第一轮摘要；challenge 以 base64 ASCII 参与第二轮。
    let mut first =
        Vec::with_capacity(device_pub_key_mod.len() + device_pub_key_exp.len() + PEPPER.len());
    first.extend_from_slice(device_pub_key_mod.as_bytes());
    first.extend_from_slice(device_pub_key_exp.as_bytes());
    first.extend_from_slice(PEPPER);
    let e = sha256(&first);
    let b = uppercase_hex(&e);
    let mut second = Vec::with_capacity(b.len() + challenge_b64.len());
    second.extend_from_slice(b.as_bytes());
    second.extend_from_slice(challenge_b64.as_bytes());
    let d = sha256(&second);
    let mut sign_key = [0u8; 32];
    for (slot, (x, y)) in sign_key.iter_mut().zip(d.iter().zip(e.iter())) {
        *slot = x ^ y;
    }
    sign_key
}

/// 同 [`derive_sign_key`]，返回 64 字符大写十六进制（官方 `SdpcPubkeyPersistence` 实体的
/// `signKey` 字段形态；前 4 个 hex 即日志 `SdpcPk save:<XXXX>` 的键前缀）。结果由零化容器
/// 持有。
#[must_use]
pub fn derive_sign_key_hex(
    device_pub_key_mod: &str,
    device_pub_key_exp: &str,
    challenge_b64: &str,
) -> Zeroizing<String> {
    uppercase_hex(&derive_sign_key(
        device_pub_key_mod,
        device_pub_key_exp,
        challenge_b64,
    ))
}

/// 从 authConfig 的 `antiMITMAttackData` 派生 signKey；三个输入字段任一缺失/为空（例如
/// 网关对未知设备未颁发设备数据，实验 B 待判）时返回 `None`，不编造回退派生
/// （A.5 记录的 `extra=""` 回退路径产物为恒定值，非官方主路径，不复刻）。
#[must_use]
pub fn derive_from_anti_mitm(data: &super::AntiMitmAttackData) -> Option<[u8; 32]> {
    let modulus = data.device_pub_key_mod.as_deref()?;
    let exponent = data.device_pub_key_exp.as_deref()?;
    let challenge = data.challenge.as_deref()?;
    if modulus.is_empty() || exponent.is_empty() || challenge.is_empty() {
        return None;
    }
    Some(derive_sign_key(modulus, exponent, challenge))
}

/// 按 A.4 计算请求签名头 `X-Request-Sig`（与 `x-sdp-signature` 同构，见模块文档）：
/// `UPPERCASE_HEX(HMAC-SHA256(signKey, path_and_query [+ body]))`。
/// `path_and_query` 是路径加查询串（不含 scheme/host/port）；`body` 非空时原文拼接在
/// uri 之后（原始字节，无规范化）。JSON 正文是 UTF-8，`&str` 字节即原始字节。
#[must_use]
pub fn request_sig(sign_key: &[u8; 32], path_and_query: &str, body: Option<&str>) -> String {
    let key = hmac::Key::new(hmac::HMAC_SHA256, sign_key);
    let mut message = Vec::with_capacity(path_and_query.len() + body.map_or(0, str::len));
    message.extend_from_slice(path_and_query.as_bytes());
    if let Some(body) = body {
        message.extend_from_slice(body.as_bytes());
    }
    // 输出是会随请求上送的签名值（非密钥材料），用普通字符串持有。
    let tag = hmac::sign(&key, &message);
    let mut signature = String::with_capacity(tag.as_ref().len() * 2);
    push_uppercase_hex(tag.as_ref(), &mut signature);
    signature
}

fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(digest::digest(&digest::SHA256, bytes).as_ref());
    out
}

/// 官方 `%02X` 终编码（A.5）：大写十六进制写入既有缓冲。
fn push_uppercase_hex(bytes: &[u8], out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in bytes {
        out.push(char::from(HEX[usize::from(byte >> 4)]));
        out.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
}

/// 摘要等派生中间值属密钥派生材料，由零化容器持有。
fn uppercase_hex(bytes: &[u8]) -> Zeroizing<String> {
    let mut encoded = Zeroizing::new(String::with_capacity(bytes.len() * 2));
    push_uppercase_hex(bytes, &mut encoded);
    encoded
}
