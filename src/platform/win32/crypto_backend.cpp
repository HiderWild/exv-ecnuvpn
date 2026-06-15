#include "platform/common/crypto_backend.hpp"

#include "observability/log_facade.hpp"

#include <windows.h>
#include <bcrypt.h>
#include <ntstatus.h>
#include <wincrypt.h>

#include <cstring>
#include <cwchar>
#include <iostream>

#ifndef NT_SUCCESS
#define NT_SUCCESS(Status) (((NTSTATUS)(Status)) >= 0)
#endif

namespace ecnuvpn {
namespace platform {
namespace {

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

bool bcrypt_open_aes_cbc(BCryptAlgRAII *alg) {
  const auto chaining_mode_bytes = static_cast<ULONG>(
      (std::wcslen(BCRYPT_CHAIN_MODE_CBC) + 1) * sizeof(wchar_t));
  NTSTATUS status = BCryptOpenAlgorithmProvider(
      &alg->handle, BCRYPT_AES_ALGORITHM, nullptr, 0);
  if (!NT_SUCCESS(status)) {
    exv::observability::LogFacade::error("BCryptOpenAlgorithmProvider failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return false;
  }

  status = BCryptSetProperty(
      alg->handle, BCRYPT_CHAINING_MODE,
      reinterpret_cast<PUCHAR>(const_cast<wchar_t *>(BCRYPT_CHAIN_MODE_CBC)),
      chaining_mode_bytes, 0);
  if (!NT_SUCCESS(status)) {
    exv::observability::LogFacade::error("BCryptSetProperty(CHAINING_MODE) failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return false;
  }

  return true;
}

bool import_aes_key(BCryptAlgRAII *alg,
                    const uint8_t *key_bytes,
                    std::size_t key_len,
                    BCryptKeyRAII *key_handle,
                    std::vector<uint8_t> *key_object) {
  const auto chaining_mode_bytes = static_cast<ULONG>(
      (std::wcslen(BCRYPT_CHAIN_MODE_CBC) + 1) * sizeof(wchar_t));
  ULONG key_object_len = 0;
  ULONG property_len = 0;
  NTSTATUS status = BCryptGetProperty(
      alg->handle, BCRYPT_OBJECT_LENGTH,
      reinterpret_cast<PUCHAR>(&key_object_len), sizeof(key_object_len),
      &property_len, 0);
  if (!NT_SUCCESS(status)) {
    exv::observability::LogFacade::error("BCryptGetProperty(OBJECT_LENGTH) failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return false;
  }

  key_object->assign(key_object_len, 0);
  status = BCryptGenerateSymmetricKey(
      alg->handle, &key_handle->handle, key_object->data(),
      static_cast<ULONG>(key_object->size()), const_cast<PUCHAR>(key_bytes),
      static_cast<ULONG>(key_len), 0);
  if (!NT_SUCCESS(status)) {
    exv::observability::LogFacade::error("BCryptGenerateSymmetricKey failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return false;
  }

  status = BCryptSetProperty(
      key_handle->handle, BCRYPT_CHAINING_MODE,
      reinterpret_cast<PUCHAR>(const_cast<wchar_t *>(BCRYPT_CHAIN_MODE_CBC)),
      chaining_mode_bytes, 0);
  if (!NT_SUCCESS(status)) {
    exv::observability::LogFacade::error("BCryptSetProperty(KEY_CHAINING_MODE) failed: 0x" +
                  std::to_string(static_cast<unsigned long>(status)));
    return false;
  }

  return true;
}

std::vector<uint8_t> apply_pkcs7_padding(const std::string &plaintext,
                                         std::size_t block_size) {
  std::vector<uint8_t> padded(plaintext.begin(), plaintext.end());
  std::size_t pad = block_size - (padded.size() % block_size);
  if (pad == 0)
    pad = block_size;
  padded.insert(padded.end(), pad, static_cast<uint8_t>(pad));
  return padded;
}

bool remove_pkcs7_padding(std::vector<uint8_t> *buffer) {
  if (!buffer || buffer->empty())
    return false;

  uint8_t pad = buffer->back();
  if (pad == 0 || pad > 16 || pad > buffer->size())
    return false;

  std::size_t start = buffer->size() - pad;
  for (std::size_t i = start; i < buffer->size(); ++i) {
    if ((*buffer)[i] != pad)
      return false;
  }

  buffer->resize(start);
  return true;
}

} // namespace

bool fill_random_bytes(uint8_t *buffer, std::size_t len) {
  NTSTATUS status = BCryptGenRandom(nullptr, buffer, static_cast<ULONG>(len),
                                    BCRYPT_USE_SYSTEM_PREFERRED_RNG);
  return NT_SUCCESS(status);
}

bool encrypt_aes256_cbc(const std::string &plaintext,
                        const uint8_t *key_bytes,
                        std::size_t key_len,
                        const uint8_t *iv,
                        std::size_t iv_len,
                        std::vector<uint8_t> *ciphertext) {
  if (!key_bytes || key_len != 32 || !iv || iv_len != 16 || !ciphertext)
    return false;

  HCRYPTPROV provider = 0;
  if (!CryptAcquireContextA(&provider, nullptr, nullptr, PROV_RSA_AES,
                            CRYPT_VERIFYCONTEXT)) {
    exv::observability::LogFacade::error("CryptAcquireContextA failed");
    return false;
  }

  struct KeyBlob {
    BLOBHEADER header;
    DWORD key_size;
    BYTE key[32];
  } blob{};
  blob.header.bType = PLAINTEXTKEYBLOB;
  blob.header.bVersion = CUR_BLOB_VERSION;
  blob.header.reserved = 0;
  blob.header.aiKeyAlg = CALG_AES_256;
  blob.key_size = 32;
  std::memcpy(blob.key, key_bytes, 32);

  HCRYPTKEY key = 0;
  if (!CryptImportKey(provider, reinterpret_cast<const BYTE *>(&blob),
                      sizeof(blob), 0, 0, &key)) {
    exv::observability::LogFacade::error("CryptImportKey failed");
    CryptReleaseContext(provider, 0);
    return false;
  }

  DWORD mode = CRYPT_MODE_CBC;
  if (!CryptSetKeyParam(key, KP_MODE, reinterpret_cast<BYTE *>(&mode), 0)) {
    exv::observability::LogFacade::error("CryptSetKeyParam(KP_MODE) failed");
    CryptDestroyKey(key);
    CryptReleaseContext(provider, 0);
    return false;
  }

  BYTE iv_local[16];
  std::memcpy(iv_local, iv, 16);
  if (!CryptSetKeyParam(key, KP_IV, iv_local, 0)) {
    exv::observability::LogFacade::error("CryptSetKeyParam(KP_IV) failed");
    CryptDestroyKey(key);
    CryptReleaseContext(provider, 0);
    return false;
  }

  std::vector<uint8_t> buffer(plaintext.begin(), plaintext.end());
  DWORD data_len = static_cast<DWORD>(buffer.size());
  DWORD max_len = data_len + 16;
  buffer.resize(max_len);

  if (!CryptEncrypt(key, 0, TRUE, 0, buffer.data(), &data_len, max_len)) {
    exv::observability::LogFacade::error("CryptEncrypt failed");
    CryptDestroyKey(key);
    CryptReleaseContext(provider, 0);
    return false;
  }

  buffer.resize(data_len);
  *ciphertext = std::move(buffer);
  CryptDestroyKey(key);
  CryptReleaseContext(provider, 0);
  return true;
}

bool decrypt_aes256_cbc(const uint8_t *ciphertext,
                        std::size_t ciphertext_len,
                        const uint8_t *key_bytes,
                        std::size_t key_len,
                        const uint8_t *iv,
                        std::size_t iv_len,
                        std::string *plaintext) {
  if (!ciphertext || ciphertext_len == 0 || !key_bytes || key_len != 32 ||
      !iv || iv_len != 16 || !plaintext)
    return false;

  HCRYPTPROV provider = 0;
  if (!CryptAcquireContextA(&provider, nullptr, nullptr, PROV_RSA_AES,
                            CRYPT_VERIFYCONTEXT)) {
    exv::observability::LogFacade::error("CryptAcquireContextA failed");
    return false;
  }

  struct KeyBlob {
    BLOBHEADER header;
    DWORD key_size;
    BYTE key[32];
  } blob{};
  blob.header.bType = PLAINTEXTKEYBLOB;
  blob.header.bVersion = CUR_BLOB_VERSION;
  blob.header.reserved = 0;
  blob.header.aiKeyAlg = CALG_AES_256;
  blob.key_size = 32;
  std::memcpy(blob.key, key_bytes, 32);

  HCRYPTKEY key = 0;
  if (!CryptImportKey(provider, reinterpret_cast<const BYTE *>(&blob),
                      sizeof(blob), 0, 0, &key)) {
    exv::observability::LogFacade::error("CryptImportKey failed");
    CryptReleaseContext(provider, 0);
    return false;
  }

  DWORD mode = CRYPT_MODE_CBC;
  if (!CryptSetKeyParam(key, KP_MODE, reinterpret_cast<BYTE *>(&mode), 0)) {
    exv::observability::LogFacade::error("CryptSetKeyParam(KP_MODE) failed");
    CryptDestroyKey(key);
    CryptReleaseContext(provider, 0);
    return false;
  }

  BYTE iv_local[16];
  std::memcpy(iv_local, iv, 16);
  if (!CryptSetKeyParam(key, KP_IV, iv_local, 0)) {
    exv::observability::LogFacade::error("CryptSetKeyParam(KP_IV) failed");
    CryptDestroyKey(key);
    CryptReleaseContext(provider, 0);
    return false;
  }

  std::vector<uint8_t> buffer(ciphertext, ciphertext + ciphertext_len);
  DWORD data_len = static_cast<DWORD>(buffer.size());
  if (!CryptDecrypt(key, 0, TRUE, 0, buffer.data(), &data_len)) {
    exv::observability::LogFacade::error("CryptDecrypt failed");
    CryptDestroyKey(key);
    CryptReleaseContext(provider, 0);
    return false;
  }

  buffer.resize(data_len);
  plaintext->assign(reinterpret_cast<const char *>(buffer.data()),
                    buffer.size());
  CryptDestroyKey(key);
  CryptReleaseContext(provider, 0);
  return true;
}

void normalize_secret_file_permissions(const std::string &) {}

std::string read_hidden_input(const std::string &prompt) {
  std::cerr << prompt;
  std::cerr.flush();

  HANDLE hStdin = GetStdHandle(STD_INPUT_HANDLE);
  DWORD old_mode = 0;
  bool console_ok = (hStdin != INVALID_HANDLE_VALUE &&
                     GetConsoleMode(hStdin, &old_mode) != 0);
  if (console_ok) {
    SetConsoleMode(hStdin, old_mode & ~ENABLE_ECHO_INPUT);
  }

  std::string password;
  std::getline(std::cin, password);

  if (console_ok) {
    SetConsoleMode(hStdin, old_mode);
  }

  std::cerr << std::endl;
  return password;
}

} // namespace platform
} // namespace ecnuvpn