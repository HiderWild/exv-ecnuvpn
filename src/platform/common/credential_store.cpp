#include "credential_store.hpp"
#include <iostream>

#ifdef _WIN32
#include "../win32/win_credential_store.hpp"
#elif defined(__APPLE__)
#include "../darwin/darwin_keychain_store.hpp"
#endif

namespace exv::platform {

// ---------------------------------------------------------------------------
// Fallback for platforms without a supported OS credential store.
// Every method produces a clear, observable signal so callers never silently
// lose secrets.
// ---------------------------------------------------------------------------

class UnsupportedCredentialStore : public CredentialStore {
public:
    bool save(const std::string& /*profile_id*/,
              const std::string& /*secret_name*/,
              const std::string& /*secret*/) override {
        std::cerr << "[CredentialStore] WARNING: save() called on unsupported "
                     "backend -- secret NOT persisted.  "
                     "Use a platform with Credential Manager / Keychain support."
                  << std::endl;
        return false;
    }

    std::optional<std::string> load(const std::string& /*profile_id*/,
                                    const std::string& /*secret_name*/) const override {
        return std::nullopt;
    }

    bool erase(const std::string& /*profile_id*/,
               const std::string& /*secret_name*/) override {
        return false;
    }

    bool is_available() const override { return false; }
    std::string backend_name() const override { return "unsupported"; }
};

// ---------------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------------

std::unique_ptr<CredentialStore> CredentialStore::create() {
#ifdef _WIN32
    return std::make_unique<win32::WinCredentialStore>();
#elif defined(__APPLE__)
    return std::make_unique<darwin::DarwinKeychainStore>();
#else
    return std::make_unique<UnsupportedCredentialStore>();
#endif
}

} // namespace exv::platform
