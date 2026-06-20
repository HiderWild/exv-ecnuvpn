#pragma once
#include "../common/credential_store.hpp"

#ifdef _WIN32

namespace exv::platform::win32 {

/// CredentialStore backed by the Windows Credential Manager
/// (CredWrite / CredRead / CredDelete).
class WinCredentialStore : public CredentialStore {
public:
    WinCredentialStore();
    ~WinCredentialStore() override;

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
    /// Build the CRED_TARGET_NAME: "EXV/<profile_id>/<secret_name>"
    std::string make_target(const std::string& profile_id,
                            const std::string& secret_name) const;
};

} // namespace exv::platform::win32

#endif // _WIN32
