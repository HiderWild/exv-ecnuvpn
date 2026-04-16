#include "crypto.hpp"
#include "logger.hpp"
#include "utils.hpp"

#include <algorithm>
#include <cstring>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <sstream>
#include <vector>

#include <nlohmann/json.hpp>

// macOS built-in — no extra brew install required
#include <CommonCrypto/CommonCryptor.h>
#include <CommonCrypto/CommonRandom.h>

// POSIX for termios hidden input
#include <sys/stat.h>
#include <termios.h>
#include <unistd.h>

namespace ecnuvpn {
namespace crypto {

// ── Internal helpers ─────────────────────────────────────────────

// Base64 table
static const char B64_TABLE[] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

static std::string base64_encode(const uint8_t *data, size_t len) {
  std::string out;
  out.reserve(((len + 2) / 3) * 4);
  for (size_t i = 0; i < len; i += 3) {
    uint32_t n = static_cast<uint32_t>(data[i]) << 16;
    if (i + 1 < len)
      n |= static_cast<uint32_t>(data[i + 1]) << 8;
    if (i + 2 < len)
      n |= static_cast<uint32_t>(data[i + 2]);
    out.push_back(B64_TABLE[(n >> 18) & 63]);
    out.push_back(B64_TABLE[(n >> 12) & 63]);
    out.push_back(i + 1 < len ? B64_TABLE[(n >> 6) & 63] : '=');
    out.push_back(i + 2 < len ? B64_TABLE[n & 63] : '=');
  }
  return out;
}

static std::vector<uint8_t> base64_decode(const std::string &encoded) {
  auto char_to_idx = [](char c) -> int {
    if (c >= 'A' && c <= 'Z')
      return c - 'A';
    if (c >= 'a' && c <= 'z')
      return c - 'a' + 26;
    if (c >= '0' && c <= '9')
      return c - '0' + 52;
    if (c == '+')
      return 62;
    if (c == '/')
      return 63;
    return -1;
  };
  std::vector<uint8_t> out;
  out.reserve(encoded.size() * 3 / 4);
  int val = 0, bits = -8;
  for (unsigned char c : encoded) {
    if (c == '=')
      break;
    int idx = char_to_idx(static_cast<char>(c));
    if (idx < 0)
      continue;
    val = (val << 6) + idx;
    bits += 6;
    if (bits >= 0) {
      out.push_back(static_cast<uint8_t>((val >> bits) & 0xFF));
      bits -= 8;
    }
  }
  return out;
}

static std::string bytes_to_hex(const uint8_t *data, size_t len) {
  std::ostringstream ss;
  for (size_t i = 0; i < len; ++i)
    ss << std::hex << std::setw(2) << std::setfill('0')
       << static_cast<int>(data[i]);
  return ss.str();
}

static bool hex_to_bytes(const std::string &hex, uint8_t *out,
                         size_t expected_len) {
  if (hex.size() != expected_len * 2)
    return false;
  for (size_t i = 0; i < expected_len; ++i) {
    auto byte_str = hex.substr(i * 2, 2);
    // validate hex chars
    for (char c : byte_str) {
      if (!isxdigit(static_cast<unsigned char>(c)))
        return false;
    }
    out[i] = static_cast<uint8_t>(std::stoul(byte_str, nullptr, 16));
  }
  return true;
}

// ── Key management ───────────────────────────────────────────────

std::string key_path() { return utils::get_config_dir() + "/.key"; }

std::string generate_key() {
  uint8_t raw[32];
  if (CCRandomGenerateBytes(raw, sizeof(raw)) != kCCSuccess) {
    // Fallback: use /dev/urandom
    std::ifstream urandom("/dev/urandom", std::ios::binary);
    urandom.read(reinterpret_cast<char *>(raw), sizeof(raw));
  }
  return bytes_to_hex(raw, sizeof(raw));
}

bool validate_key(const std::string &hex_key) {
  if (hex_key.size() != 64)
    return false;
  return std::all_of(hex_key.begin(), hex_key.end(), [](char c) {
    return isxdigit(static_cast<unsigned char>(c));
  });
}

bool save_key(const std::string &hex_key) {
  utils::ensure_dir(utils::get_config_dir());
  std::string path = key_path();
  std::ofstream ofs(path);
  if (!ofs.is_open())
    return false;
  ofs << hex_key;
  // Restrict to owner read/write only
  chmod(path.c_str(), 0600);
  return ofs.good();
}

std::string load_key() {
  std::string path = key_path();
  if (!utils::file_exists(path))
    return "";
  std::string key = utils::trim(utils::read_file(path));
  return key;
}

std::string init_key_if_needed() {
  std::string key = load_key();
  if (!validate_key(key)) {
    key = generate_key();
    save_key(key);
    logger::info("Generated new encryption key: " + key_path());
  }
  return key;
}

std::string key_status() {
  std::string path = key_path();
  if (!utils::file_exists(path))
    return "missing";
  std::string key = utils::trim(utils::read_file(path));
  return validate_key(key) ? "valid" : "corrupt";
}

bool reset_key() {
  std::cout << std::endl;
  utils::print_warning("This will generate a NEW encryption key.");
  utils::print_warning("The stored encrypted password will be CLEARED.");
  utils::print_warning(
      "You will need to run 'exv config set password' again.");
  std::cout << std::endl;
  std::cout << utils::BOLD << utils::RED << "  Confirm key reset? [y/N] "
            << utils::RESET;
  std::string answer;
  std::getline(std::cin, answer);
  if (answer.empty() || (answer[0] != 'y' && answer[0] != 'Y')) {
    utils::print_info("Key reset cancelled.");
    return false;
  }

  std::string new_key = generate_key();
  if (!save_key(new_key)) {
    utils::print_error("Failed to save new key to: " + key_path());
    logger::error("Key reset: failed to save new key");
    return false;
  }

  // Clear password ciphertext in config.json
  std::string cfg_path = utils::get_config_path();
  if (utils::file_exists(cfg_path)) {
    std::string content = utils::read_file(cfg_path);
    try {
      auto j = nlohmann::json::parse(content);
      j["password"] = "";
      utils::write_file(cfg_path, j.dump(4));
    } catch (...) {
    }
  }

  utils::print_success("Key reset successfully. Password cleared.");
  utils::print_info("Set a new password with: exv config set password");
  logger::info("Encryption key reset performed");
  return true;
}

// ── Encryption / Decryption ──────────────────────────────────────

std::string encrypt(const std::string &plaintext, const std::string &hex_key) {
  if (!validate_key(hex_key))
    return "";

  uint8_t key_bytes[32];
  if (!hex_to_bytes(hex_key, key_bytes, 32))
    return "";

  // Generate random IV
  uint8_t iv[kCCBlockSizeAES128];
  if (CCRandomGenerateBytes(iv, sizeof(iv)) != kCCSuccess) {
    std::ifstream urandom("/dev/urandom", std::ios::binary);
    urandom.read(reinterpret_cast<char *>(iv), sizeof(iv));
  }

  // Compute output buffer size (AES CBC pads to block boundary)
  size_t out_len = 0;
  size_t buf_size = plaintext.size() + kCCBlockSizeAES128;
  std::vector<uint8_t> ciphertext(buf_size);

  CCCryptorStatus status =
      CCCrypt(kCCEncrypt, kCCAlgorithmAES, kCCOptionPKCS7Padding, key_bytes,
              sizeof(key_bytes), // key + key size
              iv,                // IV
              plaintext.data(), plaintext.size(), ciphertext.data(), buf_size,
              &out_len);

  if (status != kCCSuccess) {
    logger::error("AES encrypt failed, status: " + std::to_string(status));
    return "";
  }
  ciphertext.resize(out_len);

  // Prepend IV to ciphertext, then base64 encode the whole thing
  std::vector<uint8_t> combined(sizeof(iv) + out_len);
  std::memcpy(combined.data(), iv, sizeof(iv));
  std::memcpy(combined.data() + sizeof(iv), ciphertext.data(), out_len);

  return base64_encode(combined.data(), combined.size());
}

std::string decrypt(const std::string &ciphertext_b64,
                    const std::string &hex_key) {
  if (ciphertext_b64.empty())
    return "";
  if (!validate_key(hex_key))
    return "";

  uint8_t key_bytes[32];
  if (!hex_to_bytes(hex_key, key_bytes, 32))
    return "";

  auto combined = base64_decode(ciphertext_b64);
  if (combined.size() <= kCCBlockSizeAES128) {
    logger::error("AES decrypt: ciphertext too short");
    return "";
  }

  // Split IV and ciphertext
  const uint8_t *iv = combined.data();
  const uint8_t *enc_data = combined.data() + kCCBlockSizeAES128;
  size_t enc_len = combined.size() - kCCBlockSizeAES128;

  size_t out_len = 0;
  std::vector<uint8_t> plaintext(enc_len + kCCBlockSizeAES128);

  CCCryptorStatus status =
      CCCrypt(kCCDecrypt, kCCAlgorithmAES, kCCOptionPKCS7Padding, key_bytes,
              sizeof(key_bytes), iv, enc_data, enc_len, plaintext.data(),
              plaintext.size(), &out_len);

  if (status != kCCSuccess) {
    logger::error("AES decrypt failed, status: " + std::to_string(status));
    return "";
  }

  return std::string(reinterpret_cast<char *>(plaintext.data()), out_len);
}

// ── Secure hidden input ──────────────────────────────────────────

std::string read_password_hidden(const std::string &prompt) {
  // Print prompt to stderr so it shows even when stdout is redirected
  std::cerr << prompt;
  std::cerr.flush();

  // Disable echo via termios
  struct termios old_termios, new_termios;
  bool tty = (tcgetattr(STDIN_FILENO, &old_termios) == 0);
  if (tty) {
    new_termios = old_termios;
    new_termios.c_lflag &= ~(ECHO | ECHOE | ECHOK | ECHONL);
    tcsetattr(STDIN_FILENO, TCSANOW, &new_termios);
  }

  std::string password;
  std::getline(std::cin, password);

  // Restore echo
  if (tty) {
    tcsetattr(STDIN_FILENO, TCSANOW, &old_termios);
  }

  // Print newline (since echo was off, enter key didn't produce one)
  std::cerr << std::endl;

  return password;
}

} // namespace crypto
} // namespace ecnuvpn
