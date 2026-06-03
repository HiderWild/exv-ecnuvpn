#include "win_credential_store.hpp"

#ifdef _WIN32

#include <windows.h>
#include <wincred.h>
#include <vector>
#include <stdexcept>

#pragma comment(lib, "advapi32.lib")

namespace exv::platform::win32 {

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

namespace {

/// Convert a UTF-8 string to a heap-allocated wide-character buffer.
std::vector<wchar_t> to_wide(const std::string& utf8) {
    if (utf8.empty()) return {L'\0'};
    int len = MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), -1, nullptr, 0);
    if (len <= 0) throw std::runtime_error("MultiByteToWideChar failed");
    std::vector<wchar_t> buf(len);
    MultiByteToWideChar(CP_UTF8, 0, utf8.c_str(), -1, buf.data(), len);
    return buf;
}

} // anonymous namespace

// ---------------------------------------------------------------------------
// Lifetime
// ---------------------------------------------------------------------------

WinCredentialStore::WinCredentialStore() = default;
WinCredentialStore::~WinCredentialStore() = default;

// ---------------------------------------------------------------------------
// Target naming
// ---------------------------------------------------------------------------

std::string WinCredentialStore::make_target(const std::string& profile_id,
                                            const std::string& secret_name) const {
    return "ECNU-VPN/" + profile_id + "/" + secret_name;
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

bool WinCredentialStore::save(const std::string& profile_id,
                              const std::string& secret_name,
                              const std::string& secret) {
    auto wide_target = to_wide(make_target(profile_id, secret_name));

    CREDENTIALW cred = {};
    cred.Type              = CRED_TYPE_GENERIC;
    cred.TargetName        = wide_target.data();
    cred.CredentialBlobSize = static_cast<DWORD>(secret.size());
    cred.CredentialBlob    = reinterpret_cast<LPBYTE>(const_cast<char*>(secret.data()));
    cred.Persist           = CRED_PERSIST_LOCAL_MACHINE;

    return CredWriteW(&cred, 0) != FALSE;
}

std::optional<std::string> WinCredentialStore::load(const std::string& profile_id,
                                                     const std::string& secret_name) const {
    auto wide_target = to_wide(make_target(profile_id, secret_name));

    PCREDENTIALW cred = nullptr;
    if (!CredReadW(wide_target.data(), CRED_TYPE_GENERIC, 0, &cred)) {
        return std::nullopt;
    }

    std::string result(reinterpret_cast<char*>(cred->CredentialBlob),
                       cred->CredentialBlobSize);
    CredFree(cred);
    return result;
}

bool WinCredentialStore::erase(const std::string& profile_id,
                               const std::string& secret_name) {
    auto wide_target = to_wide(make_target(profile_id, secret_name));
    return CredDeleteW(wide_target.data(), CRED_TYPE_GENERIC, 0) != FALSE;
}

// ---------------------------------------------------------------------------
// Introspection
// ---------------------------------------------------------------------------

bool WinCredentialStore::is_available() const {
    // Credential Manager is always present on supported Windows versions.
    return true;
}

std::string WinCredentialStore::backend_name() const {
    return "windows-credential-manager";
}

} // namespace exv::platform::win32

#endif // _WIN32
