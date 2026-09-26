//! Windows 兼容的 AES-256-GCM 密码格式。

use aes_gcm::aead::{Aead, Payload};
use aes_gcm::{Aes256Gcm, Key, KeyInit, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::DarwinConfigError;

const NONCE_LEN: usize = 12;
const TAG_LEN: usize = 16;
pub(crate) const KEY_LEN: usize = 32;

/// 返回 Windows 同款 `base64(nonce || tag || ciphertext)`。
pub(crate) fn encrypt_password(
    plaintext: &str,
    key: &[u8; KEY_LEN],
) -> Result<String, DarwinConfigError> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let mut nonce_bytes = [0_u8; NONCE_LEN];
    getrandom::fill(&mut nonce_bytes).map_err(|_| DarwinConfigError::Storage)?;
    let encrypted = cipher
        .encrypt(
            Nonce::from_slice(&nonce_bytes),
            Payload::from(plaintext.as_bytes()),
        )
        .map_err(|_| DarwinConfigError::PasswordCrypto)?;
    let (ciphertext, tag) = encrypted.split_at(encrypted.len() - TAG_LEN);
    let mut blob = Vec::with_capacity(NONCE_LEN + TAG_LEN + ciphertext.len());
    blob.extend_from_slice(&nonce_bytes);
    blob.extend_from_slice(tag);
    blob.extend_from_slice(ciphertext);
    Ok(BASE64.encode(blob))
}

pub(crate) fn decrypt_password(
    blob_b64: &str,
    key: &[u8; KEY_LEN],
) -> Result<String, DarwinConfigError> {
    let blob = BASE64
        .decode(blob_b64)
        .map_err(|_| DarwinConfigError::PasswordCrypto)?;
    if blob.len() < NONCE_LEN + TAG_LEN {
        return Err(DarwinConfigError::PasswordCrypto);
    }
    let nonce = Nonce::from_slice(&blob[..NONCE_LEN]);
    let tag = &blob[NONCE_LEN..NONCE_LEN + TAG_LEN];
    let ciphertext = &blob[NONCE_LEN + TAG_LEN..];
    let mut encrypted = Vec::with_capacity(ciphertext.len() + TAG_LEN);
    encrypted.extend_from_slice(ciphertext);
    encrypted.extend_from_slice(tag);
    let plaintext = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key))
        .decrypt(nonce, Payload::from(encrypted.as_slice()))
        .map_err(|_| DarwinConfigError::PasswordCrypto)?;
    String::from_utf8(plaintext).map_err(|_| DarwinConfigError::PasswordCrypto)
}

pub(crate) fn generate_key() -> Result<[u8; KEY_LEN], DarwinConfigError> {
    let mut key = [0_u8; KEY_LEN];
    getrandom::fill(&mut key).map_err(|_| DarwinConfigError::Storage)?;
    Ok(key)
}
