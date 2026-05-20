#include "platform/common/crypto_backend.hpp"

#include "logger.hpp"

#include <openssl/evp.h>
#include <openssl/rand.h>

#include <fstream>
#include <iostream>
#include <sys/stat.h>
#include <termios.h>
#include <unistd.h>

namespace ecnuvpn {
namespace platform {

bool fill_random_bytes(uint8_t *buffer, std::size_t len) {
  if (RAND_bytes(buffer, static_cast<int>(len)) == 1)
    return true;

  std::ifstream urandom("/dev/urandom", std::ios::binary);
  urandom.read(reinterpret_cast<char *>(buffer), static_cast<std::streamsize>(len));
  return urandom.good();
}

bool encrypt_aes256_cbc(const std::string &plaintext,
                        const uint8_t *key_bytes,
                        std::size_t,
                        const uint8_t *iv,
                        std::size_t,
                        std::vector<uint8_t> *ciphertext) {
  EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
  if (!ctx) {
    logger::error("AES encrypt: EVP_CIPHER_CTX_new failed");
    return false;
  }

  ciphertext->assign(plaintext.size() + 16, 0);
  int out_len1 = 0;
  int out_len2 = 0;

  if (EVP_EncryptInit_ex(ctx, EVP_aes_256_cbc(), nullptr, key_bytes, iv) != 1 ||
      EVP_EncryptUpdate(ctx, ciphertext->data(), &out_len1,
                        reinterpret_cast<const uint8_t *>(plaintext.data()),
                        static_cast<int>(plaintext.size())) != 1 ||
      EVP_EncryptFinal_ex(ctx, ciphertext->data() + out_len1, &out_len2) != 1) {
    logger::error("AES encrypt failed (OpenSSL EVP)");
    EVP_CIPHER_CTX_free(ctx);
    return false;
  }

  EVP_CIPHER_CTX_free(ctx);
  ciphertext->resize(static_cast<std::size_t>(out_len1 + out_len2));
  return true;
}

bool decrypt_aes256_cbc(const uint8_t *ciphertext,
                        std::size_t ciphertext_len,
                        const uint8_t *key_bytes,
                        std::size_t,
                        const uint8_t *iv,
                        std::size_t,
                        std::string *plaintext) {
  EVP_CIPHER_CTX *ctx = EVP_CIPHER_CTX_new();
  if (!ctx) {
    logger::error("AES decrypt: EVP_CIPHER_CTX_new failed");
    return false;
  }

  std::vector<uint8_t> buffer(ciphertext_len + 16, 0);
  int out_len1 = 0;
  int out_len2 = 0;

  if (EVP_DecryptInit_ex(ctx, EVP_aes_256_cbc(), nullptr, key_bytes, iv) != 1 ||
      EVP_DecryptUpdate(ctx, buffer.data(), &out_len1, ciphertext,
                        static_cast<int>(ciphertext_len)) != 1 ||
      EVP_DecryptFinal_ex(ctx, buffer.data() + out_len1, &out_len2) != 1) {
    logger::error("AES decrypt failed (OpenSSL EVP)");
    EVP_CIPHER_CTX_free(ctx);
    return false;
  }

  EVP_CIPHER_CTX_free(ctx);
  plaintext->assign(reinterpret_cast<char *>(buffer.data()),
                    static_cast<std::size_t>(out_len1 + out_len2));
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
} // namespace ecnuvpn