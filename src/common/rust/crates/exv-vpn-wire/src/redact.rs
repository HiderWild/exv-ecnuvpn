
/// Render a secret byte payload in a way that can never leak its contents, a
/// native handle, or a raw certificate. The entire payload is sensitive: the
/// render is a fixed redacted marker, never a transformation that echoes any
/// part of the input.
pub fn redact_secret(secret: &[u8]) -> String {
    if secret.is_empty() {
        return String::new();
    }
    // Fixed marker only: no password/secret, native-handle, or raw-certificate
    // material can appear in the rendered form.
    String::from("<redacted>")
}

