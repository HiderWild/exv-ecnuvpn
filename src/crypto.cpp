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

// Platform-specific crypto
#ifdef __APPLE__
#include <CommonCrypto/CommonCryptor.h>
#include <CommonCrypto/CommonRandom.h>
#elif defined(_WIN32)
#include <windows.h>
#include <bcrypt.h>
#include <ntstatus.h>
#ifndef NT_SUCCESS
#define NT_SUCCESS(Status) (((NTSTATUS)(Status)) >= 0)
#endif
#elif defined(__linux__)
#include <openssl/evp.h>
#include <openssl/rand.h>
#endif

// Platform-specific hidden input
#ifndef _WIN32
#include <sys/stat.h>
#include <termios.h>
#include <unistd.h>
#else
#include <conio.h>
#endif

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

static bool fill_random_bytes(uint8_t *buffer, size_t len) {
#ifdef __APPLE__
  if (CCRandomGenerateBytes(buffer, len) == kCCSuccess)
    return true;
  std::ifstream urandom("/dev/urandom", std::ios::binary);
  urandom.read(reinterpret_cast<char *>(buffer), static_cast<std::streamsize>(len));
  return urandom.good();
#elif defined(_WIN32)
  NTSTATUS status = BCryptGenRandom(nullptr, buffer, static_cast<ULONG>(len),
                                    BCRYPT_USE_SYSTEM_PREFERRED_RNG);
  return NT_SUCCESS(status);
#else
  if (RAND_bytes(buffer, static_cast<int>(len)) == 1)
    return true;
  std::ifstream urandom("/dev/urandom", std::ios::binary);
  urandom.read(reinterpret_cast<char *>(buffer), static_cast<std::streamsize>(len));
  return urandom.good();
#endif
}

std::string generate_key() {
  uint8_t raw[32];
  if (!fill_random_bytes(raw, sizeof(raw))) {
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
#ifndef _WIN32
  // Restrict to owner read/write only
  chmod(path.c_str(), 0600);
#endif
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

#ifdef _WIN32
// RAII wrappers for CNG handles
struct BCryptAlgRAII {
  BCRYPT_ALG_HANDLE handle = nullptr;
  ~BCryptAlgRAII() {
    if (handle)
      BCryptCloseAlgorithmProvider(handle, 0);
  }
};

struct BCryptKeyRAII {
  BCRYPT_KEY_HANDLE handle = nullptr;
  ~BCryptKeyRAII() {
    if (handle)
      BCryptDestroyKey(handle);
  }
};

static bool bcrypt_open_aes_cbc(BCryptAlgRAII *alg) {
  NTSTATUS status = BCryptOpenAlgorithmProvider(
      &alg->handle, BCRYPT_AES_ALGORITHM, nullptr, 0);
  if (!NT_SUCCESS(status)) {
    logger::error("BCryptOpenAlgorithmProvider failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return false;
  }
  status = BCryptSetProperty(
      alg->handle, BCRYPT_CHAINING_MODE,
      reinterpret_cast<PUCHAR>(const_cast<wchar_t *>(BCRYPT_CHAIN_MODE_CBC)),
      sizeof(BCRYPT_CHAIN_MODE_CBC), 0);
  if (!NT_SUCCESS(status)) {
    logger::error("BCryptSetProperty(CHAINING_MODE) failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return false;
  }
  return true;
}
#endif

std::string encrypt(const std::string &plaintext, const std::string &hex_key) {
  if (!validate_key(hex_key))
    return "";

  uint8_t key_bytes[32];
  if (!hex_to_bytes(hex_key, key_bytes, 32))
    return "";

  constexpr size_t IV_LEN = 16;

  // Generate random IV
  uint8_t iv[IV_LEN];
  if (!fill_random_bytes(iv, sizeof(iv))) {
    logger::error("AES encrypt: failed to gather random IV");
    return "";
  }

  // Encrypt
  size_t out_len = 0;
  std::vector<uint8_t> ciphertext;

#ifdef __APPLE__
  size_t buf_size = plaintext.size() + kCCBlockSizeAES128;
  ciphertext.resize(buf_size);

  CCCryptorStatus status =
      CCCrypt(kCCEncrypt, kCCAlgorithmAES, kCCOptionPKCS7Padding, key_bytes,
              sizeof(key_bytes), iv,
              plaintext.data(), plaintext.size(), ciphertext.data(), buf_size,
              &out_len);

  if (status != kCCSuccess) {
    logger::error("AES encrypt failed, status: " + std::to_string(status));
    return "";
  }
  ciphertext.resize(out_len);
#elif defined(_WIN32)
  BCryptAlgRAII alg;
  if (!bcrypt_open_aes_cbc(&alg))
    return "";

  BCryptKeyRAII key_handle;
  NTSTATUS status = BCryptGenerateSymmetricKey(
      alg.handle, &key_handle.handle, nullptr, 0, key_bytes, 32, 0);
  if (!NT_SUCCESS(status)) {
    logger::error("BCryptGenerateSymmetricKey failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return "";
  }

  ULONG cbResult = 0;
  uint8_t iv_copy[IV_LEN];
  std::memcpy(iv_copy, iv, IV_LEN);

  status = BCryptEncrypt(
      key_handle.handle,
      reinterpret_cast<PUCHAR>(const_cast<char *>(plaintext.data())),
      static_cast<ULONG>(plaintext.size()), nullptr, iv_copy, IV_LEN, nullptr,
      0, &cbResult, BCRYPT_BLOCK_PADDING);
  if (!NT_SUCCESS(status)) {
    logger::error("BCryptEncrypt (size query) failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return "";
  }

  ciphertext.resize(cbResult);
  std::memcpy(iv_copy, iv, IV_LEN);
  status = BCryptEncrypt(
      key_handle.handle,
      reinterpret_cast<PUCHAR>(const_cast<char *>(plaintext.data())),
      static_cast<ULONG>(plaintext.size()), nullptr, iv_copy, IV_LEN,
      ciphertext.data(), static_cast<ULONG>(ciphertext.size()), &cbResult,
      BCRYPT_BLOCK_PADDING);
  if (!NT_SUCCESS(status)) {
    logger::error("BCryptEncrypt failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return "";
  }
  out_len = static_cast<size_t>(cbResult);
  ciphertext.resize(out_len);
#else
  EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
  if (!ctx) {
    logger::error("AES encrypt: EVP_CIPHER_CTX_new failed");
    return "";
  }

  ciphertext.resize(plaintext.size() + IV_LEN);
  int out_len1 = 0, out_len2 = 0;

  if (EVP_EncryptInit_ex(ctx, EVP_aes_256_cbc(), nullptr, key_bytes, iv) != 1 ||
      EVP_EncryptUpdate(ctx, ciphertext.data(), &out_len1,
                        reinterpret_cast<const uint8_t *>(plaintext.data()),
                        static_cast<int>(plaintext.size())) != 1 ||
      EVP_EncryptFinal_ex(ctx, ciphertext.data() + out_len1, &out_len2) != 1) {
    logger::error("AES encrypt failed (OpenSSL EVP)");
    EVP_CIPHER_CTX_free(ctx);
    return "";
  }
  EVP_CIPHER_CTX_free(ctx);
  out_len = static_cast<size_t>(out_len1 + out_len2);
  ciphertext.resize(out_len);
#endif

  // Prepend IV to ciphertext, then base64 encode the whole thing
  std::vector<uint8_t> combined(IV_LEN + out_len);
  std::memcpy(combined.data(), iv, IV_LEN);
  std::memcpy(combined.data() + IV_LEN, ciphertext.data(), out_len);

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

  // Split IV and ciphertext
  const uint8_t *iv = combined.data();
  const uint8_t *enc_data = combined.data() + IV_LEN;
  size_t enc_len = combined.size() - IV_LEN;

#ifdef __APPLE__
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
#elif defined(_WIN32)
  BCryptAlgRAII alg;
  if (!bcrypt_open_aes_cbc(&alg))
    return "";

  BCryptKeyRAII key_handle;
  NTSTATUS status = BCryptGenerateSymmetricKey(
      alg.handle, &key_handle.handle, nullptr, 0, key_bytes, 32, 0);
  if (!NT_SUCCESS(status)) {
    logger::error("BCryptGenerateSymmetricKey (decrypt) failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return "";
  }

  std::vector<uint8_t> plaintext(enc_len);
  uint8_t iv_copy[IV_LEN];
  std::memcpy(iv_copy, iv, IV_LEN);
  ULONG cbResult = 0;

  status = BCryptDecrypt(
      key_handle.handle, const_cast<PUCHAR>(enc_data),
      static_cast<ULONG>(enc_len), nullptr, iv_copy, IV_LEN, plaintext.data(),
      static_cast<ULONG>(plaintext.size()), &cbResult, BCRYPT_BLOCK_PADDING);
  if (!NT_SUCCESS(status)) {
    logger::error("BCryptDecrypt failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return "";
  }

  return std::string(reinterpret_cast<char *>(plaintext.data()),
                     static_cast<size_t>(cbResult));
#else
  EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
  if (!ctx) {
    logger::error("AES decrypt: EVP_CIPHER_CTX_new failed");
    return "";
  }

  std::vector<uint8_t> plaintext(enc_len + IV_LEN);
  int out_len1 = 0, out_len2 = 0;

  if (EVP_DecryptInit_ex(ctx, EVP_aes_256_cbc(), nullptr, key_bytes, iv) != 1 ||
      EVP_DecryptUpdate(ctx, plaintext.data(), &out_len1,
                        enc_data, static_cast<int>(enc_len)) != 1 ||
      EVP_DecryptFinal_ex(ctx, plaintext.data() + out_len1, &out_len2) != 1) {
    logger::error("AES decrypt failed (OpenSSL EVP)");
    EVP_CIPHER_CTX_free(ctx);
    return "";
  }
  EVP_CIPHER_CTX_free(ctx);

  return std::string(reinterpret_cast<char *>(plaintext.data()),
                     static_cast<size_t>(out_len1 + out_len2));
#endif
}

// ── Secure hidden input ──────────────────────────────────────────

std::string read_password_hidden(const std::string &prompt) {
  // Print prompt to stderr so it shows even when stdout is redirected
  std::cerr << prompt;
  std::cerr.flush();

#ifndef _WIN32
  // POSIX: disable echo via termios
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
#else
  // Windows: disable echo via SetConsoleMode
  HANDLE hStdin = GetStdHandle(STD_INPUT_HANDLE);
  DWORD old_mode = 0;
  bool console_ok = (hStdin != INVALID_HANDLE_VALUE &&
                     GetConsoleMode(hStdin, &old_mode) != 0);
  if (console_ok) {
    SetConsoleMode(hStdin, old_mode & ~ENABLE_ECHO_INPUT);
  }

  std::string password;
  std::getline(std::cin, password);

  // Restore console mode
  if (console_ok) {
    SetConsoleMode(hStdin, old_mode);
  }
#endif

  // Print newline (since echo was off, enter key didn't produce one)
  std::cerr << std::endl;

  return password;
}

} // namespace crypto
} // namespace ecnuvpn
