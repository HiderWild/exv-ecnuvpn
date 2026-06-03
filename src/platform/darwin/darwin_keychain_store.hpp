#pragma once
#include "../common/credential_store.hpp"

#ifdef __APPLE__

namespace exv::platform::darwin {

/// CredentialStore backed by the macOS Keychain
/// (SecKeychainAddGenericPassword / SecKeychainFindGenericPassword /
///  SecKeychainItemDelete).
class DarwinKeychainStore : public CredentialStore {
public:
    bool save(const std::string& profile_id,
              const std::string& secret_name,
              const std::string& secret) override;

    std::optional<std::string> load(const std::string& profile_id,
                                    const std::string& secret_name) const override;

    bool erase(const std::string& profile_id,
               const std::string& secret_name) override;

    bool is_available() const override;
    std::string backend_name() const override;

private:
    /// Keychain "service" name: "com.ecnu-vpn.profile.<profile_id>"
    std::string make_service(const std::string& profile_id) const;

    /// Keychain "account" name: "<profile_id>/<secret_name>"
    std::string make_account(const std::string& profile_id,
                             const std::string& secret_name) const;
};

} // namespace exv::platform::darwin

#endif // __APPLE__
