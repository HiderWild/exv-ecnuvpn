#pragma once
#include <string>
#include <optional>
#include <memory>

namespace exv::platform {

/// Abstract interface for OS-backed secret storage.
///
/// Implementations encrypt at rest via the platform's native credential
/// subsystem (Windows Credential Manager, macOS Keychain).  On platforms
/// without a supported store the factory returns an UnsupportedCredentialStore
/// whose methods log a clear warning and return structured failure values.
class CredentialStore {
public:
    virtual ~CredentialStore() = default;

    /// Persist `secret` under (profile_id, secret_name).
    /// Returns true on success.
    virtual bool save(const std::string& profile_id,
                      const std::string& secret_name,
                      const std::string& secret) = 0;

    /// Retrieve a previously saved secret.  Returns std::nullopt when
    /// the entry does not exist or the backend is unavailable.
    virtual std::optional<std::string> load(const std::string& profile_id,
                                            const std::string& secret_name) const = 0;

    /// Delete the secret identified by (profile_id, secret_name).
    /// Returns false when the entry does not exist.
    virtual bool erase(const std::string& profile_id,
                       const std::string& secret_name) = 0;

    /// Whether the backend can actually store secrets.
    virtual bool is_available() const = 0;

    /// Human-readable backend identifier (e.g. "windows-credential-manager").
    virtual std::string backend_name() const = 0;

    /// Factory: returns the best platform-specific implementation, or an
    /// UnsupportedCredentialStore if none is available.
    static std::unique_ptr<CredentialStore> create();
};

} // namespace exv::platform
