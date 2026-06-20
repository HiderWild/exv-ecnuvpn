#include "platform/common/crypto_backend.hpp"

#include "observability/log_facade.hpp"

#include <CommonCrypto/CommonCryptor.h>
#include <CommonCrypto/CommonRandom.h>

#include <fstream>
#include <iostream>
#include <sys/stat.h>
#include <termios.h>
#include <unistd.h>

namespace exv {
namespace platform {

bool fill_random_bytes(uint8_t *buffer, std::size_t len) {
  if (CCRandomGenerateBytes(buffer, len) == kCCSuccess)
    return true;

  std::ifstream urandom("/dev/urandom", std::ios::binary);
  urandom.read(reinterpret_cast<char *>(buffer), static_cast<std::streamsize>(len));
  return urandom.good();
}

bool encrypt_aes256_cbc(const std::string &plaintext,
                        const uint8_t *key_bytes,
                        std::size_t key_len,
                        const uint8_t *iv,
                        std::size_t,
                        std::vector<uint8_t> *ciphertext) {
  std::size_t out_len = 0;
  ciphertext->assign(plaintext.size() + kCCBlockSizeAES128, 0);

  CCCryptorStatus status = CCCrypt(
      kCCEncrypt, kCCAlgorithmAES, kCCOptionPKCS7Padding, key_bytes, key_len,
      iv, plaintext.data(), plaintext.size(), ciphertext->data(),
      ciphertext->size(), &out_len);

  if (status != kCCSuccess) {
    exv::observability::LogFacade::error("AES encrypt failed, status: " + std::to_string(status));
    return false;
  }

  ciphertext->resize(out_len);
  return true;
}

bool decrypt_aes256_cbc(const uint8_t *ciphertext,
                        std::size_t ciphertext_len,
                        const uint8_t *key_bytes,
                        std::size_t key_len,
                        const uint8_t *iv,
                        std::size_t,
                        std::string *plaintext) {
  std::size_t out_len = 0;
  std::vector<uint8_t> buffer(ciphertext_len + kCCBlockSizeAES128, 0);

  CCCryptorStatus status = CCCrypt(
      kCCDecrypt, kCCAlgorithmAES, kCCOptionPKCS7Padding, key_bytes, key_len,
      iv, ciphertext, ciphertext_len, buffer.data(), buffer.size(), &out_len);

  if (status != kCCSuccess) {
    exv::observability::LogFacade::error("AES decrypt failed, status: " + std::to_string(status));
    return false;
  }

  plaintext->assign(reinterpret_cast<char *>(buffer.data()), out_len);
  return true;
}

void normalize_secret_file_permissions(const std::string &path) {
  chmod(path.c_str(), 0600);
}

std::string read_hidden_input(const std::string &prompt) {
  std::cerr << prompt;
  std::cerr.flush();

  struct termios old_termios, new_termios;
  bool tty = (tcgetattr(STDIN_FILENO, &old_termios) == 0);
  if (tty) {
    new_termios = old_termios;
    new_termios.c_lflag &= ~(ECHO | ECHOE | ECHOK | ECHONL);
    tcsetattr(STDIN_FILENO, TCSANOW, &new_termios);
  }

  std::string password;
  std::getline(std::cin, password);

  if (tty) {
    tcsetattr(STDIN_FILENO, TCSANOW, &old_termios);
  }

  std::cerr << std::endl;
  return password;
}

} // namespace platform
} // namespace exv