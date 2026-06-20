#pragma once

#include <cstddef>
#include <cstdint>
#include <string>
#include <vector>

namespace exv {
namespace platform {

bool fill_random_bytes(uint8_t *buffer, std::size_t len);
bool encrypt_aes256_cbc(const std::string &plaintext,
                        const uint8_t *key_bytes,
                        std::size_t key_len,
                        const uint8_t *iv,
                        std::size_t iv_len,
                        std::vector<uint8_t> *ciphertext);
bool decrypt_aes256_cbc(const uint8_t *ciphertext,
                        std::size_t ciphertext_len,
                        const uint8_t *key_bytes,
                        std::size_t key_len,
                        const uint8_t *iv,
                        std::size_t iv_len,
                        std::string *plaintext);
void normalize_secret_file_permissions(const std::string &path);
std::string read_hidden_input(const std::string &prompt);

} // namespace platform
} // namespace exv