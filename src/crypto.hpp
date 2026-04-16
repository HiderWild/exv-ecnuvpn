#pragma once

#include <string>

namespace ecnuvpn {
namespace crypto {

// ── Key management ───────────────────────────────────────────────

// Generate a new 32-byte random key, returned as 64 hex chars
std::string generate_key();

// Load key from ~/.ecnuvpn/.key (returns "" if missing/corrupt)
std::string load_key();

// Save key to ~/.ecnuvpn/.key with 0600 permissions
bool save_key(const std::string &hex_key);

// Validate: must be exactly 64 valid hex characters (32 bytes)
bool validate_key(const std::string &hex_key);

// Ensure key exists — generate and save if not. Returns the key.
std::string init_key_if_needed();

// Regenerate key, clear encrypted password from config.json.
// Returns false if user cancels.
bool reset_key();

// Human-readable status: "valid", "missing", or "corrupt"
std::string key_status();

// Path of the key file
std::string key_path();

// ── Encryption / decryption ──────────────────────────────────────

// AES-256-CBC encrypt. Returns base64(IV[16] + ciphertext).
// Returns "" on failure.
std::string encrypt(const std::string &plaintext, const std::string &hex_key);

// AES-256-CBC decrypt from base64(IV[16] + ciphertext).
// Returns "" on failure.
std::string decrypt(const std::string &ciphertext_b64,
                    const std::string &hex_key);

// ── Secure input ────────────────────────────────────────────────

// Read password from terminal with echo disabled (termios).
// Prints prompt, returns input string. Prints newline after entry.
std::string read_password_hidden(const std::string &prompt);

} // namespace crypto
} // namespace ecnuvpn
