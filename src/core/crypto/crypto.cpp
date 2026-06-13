#include "crypto.hpp"
#include "logger.hpp"
#include "platform/common/crypto_backend.hpp"
#include "utils.hpp"

#include <algorithm>
#include <cstring>
#include <cstdio>
#include <fstream>
#include <iomanip>
#include <iostream>
#include <sstream>
#include <vector>

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace crypto {
namespace {

static const char B64_TABLE[] =
    "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

std::string base64_encode(const uint8_t *data, size_t len) {
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

std::vector<uint8_t> base64_decode(const std::string &encoded) {
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
  int val = 0;
  int bits = -8;
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

std::string bytes_to_hex(const uint8_t *data, size_t len) {
  std::ostringstream ss;
  for (size_t i = 0; i < len; ++i)
    ss << std::hex << std::setw(2) << std::setfill('0')
       << static_cast<int>(data[i]);
  return ss.str();
}

bool hex_to_bytes(const std::string &hex, uint8_t *out, size_t expected_len) {
  if (hex.size() != expected_len * 2)
    return false;
  for (size_t i = 0; i < expected_len; ++i) {
    auto byte_str = hex.substr(i * 2, 2);
    for (char c : byte_str) {
      if (!isxdigit(static_cast<unsigned char>(c)))
        return false;
    }
    out[i] = static_cast<uint8_t>(std::stoul(byte_str, nullptr, 16));
  }
  return true;
}

} // namespace

std::string key_path() { return utils::get_config_dir() + "/.key"; }

std::string generate_key() {
  uint8_t raw[32];
  if (!platform::fill_random_bytes(raw, sizeof(raw))) {
    logger::error("generate_key: random source failure");
    return "";
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
  platform::normalize_secret_file_permissions(path);
  return ofs.good();
}

std::string load_key() {
  std::string path = key_path();
  if (!utils::file_exists(path))
    return "";
  return utils::trim(utils::read_file(path));
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

bool delete_key_file() {
  std::string path = key_path();
  if (!utils::file_exists(path))
    return true;
  if (std::remove(path.c_str()) == 0) {
    logger::info("Deleted encryption key: " + path);
    return true;
  }
  logger::error("Failed to delete encryption key: " + path);
  return false;
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

std::string encrypt(const std::string &plaintext, const std::string &hex_key) {
  if (!validate_key(hex_key))
    return "";

  uint8_t key_bytes[32];
  if (!hex_to_bytes(hex_key, key_bytes, 32))
    return "";

  constexpr size_t IV_LEN = 16;
  uint8_t iv[IV_LEN];
  if (!platform::fill_random_bytes(iv, sizeof(iv))) {
    logger::error("AES encrypt: failed to gather random IV");
    return "";
  }

  std::vector<uint8_t> ciphertext;
  if (!platform::encrypt_aes256_cbc(plaintext, key_bytes, sizeof(key_bytes),
                                    iv, sizeof(iv), &ciphertext)) {
    return "";
  }

  std::vector<uint8_t> combined(IV_LEN + ciphertext.size());
  std::memcpy(combined.data(), iv, IV_LEN);
  std::memcpy(combined.data() + IV_LEN, ciphertext.data(), ciphertext.size());
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

  constexpr size_t IV_LEN = 16;
  auto combined = base64_decode(ciphertext_b64);
  if (combined.size() <= IV_LEN) {
    logger::error("AES decrypt: ciphertext too short");
    return "";
  }

  const uint8_t *iv = combined.data();
  const uint8_t *enc_data = combined.data() + IV_LEN;
  size_t enc_len = combined.size() - IV_LEN;

  std::string plaintext;
  if (!platform::decrypt_aes256_cbc(enc_data, enc_len, key_bytes,
                                    sizeof(key_bytes), iv, IV_LEN,
                                    &plaintext)) {
    return "";
  }
  return plaintext;
}

std::string read_password_hidden(const std::string &prompt) {
  return platform::read_hidden_input(prompt);
}

} // namespace crypto
} // namespace ecnuvpn
