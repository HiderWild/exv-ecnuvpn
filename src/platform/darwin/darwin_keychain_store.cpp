#include "darwin_keychain_store.hpp"

#ifdef __APPLE__

#include <Security/Security.h>
#include <CoreFoundation/CoreFoundation.h>

namespace exv::platform::darwin {

// ---------------------------------------------------------------------------
// Key naming
// ---------------------------------------------------------------------------

std::string DarwinKeychainStore::make_service(const std::string& profile_id) const {
    return "com.ecnu-vpn.profile." + profile_id;
}

std::string DarwinKeychainStore::make_account(const std::string& profile_id,
                                               const std::string& secret_name) const {
    return profile_id + "/" + secret_name;
}

// ---------------------------------------------------------------------------
// CRUD
// ---------------------------------------------------------------------------

bool DarwinKeychainStore::save(const std::string& profile_id,
                               const std::string& secret_name,
                               const std::string& secret) {
    std::string service = make_service(profile_id);
    std::string account = make_account(profile_id, secret_name);

    // Remove any pre-existing entry so we can re-add with the new value.
    erase(profile_id, secret_name);

    OSStatus status = SecKeychainAddGenericPassword(
        nullptr,                                     // default keychain
        static_cast<UInt32>(service.size()), service.c_str(),
        static_cast<UInt32>(account.size()), account.c_str(),
        static_cast<UInt32>(secret.size()),  secret.data(),
        nullptr                                      // no item ref needed
    );

    return status == errSecSuccess;
}

std::optional<std::string> DarwinKeychainStore::load(const std::string& profile_id,
                                                      const std::string& secret_name) const {
    std::string service = make_service(profile_id);
    std::string account = make_account(profile_id, secret_name);

    void*   data     = nullptr;
    UInt32  data_len = 0;

    OSStatus status = SecKeychainFindGenericPassword(
        nullptr,
        static_cast<UInt32>(service.size()), service.c_str(),
        static_cast<UInt32>(account.size()), account.c_str(),
        &data_len, &data,
        nullptr
    );

    if (status != errSecSuccess || data == nullptr) {
        return std::nullopt;
    }

    std::string result(static_cast<char*>(data), data_len);
    SecKeychainItemFreeContent(nullptr, data);
    return result;
}

bool DarwinKeychainStore::erase(const std::string& profile_id,
                                const std::string& secret_name) {
    std::string service = make_service(profile_id);
    std::string account = make_account(profile_id, secret_name);

    void*            data     = nullptr;
    UInt32           data_len = 0;
    SecKeychainItemRef item    = nullptr;

    OSStatus status = SecKeychainFindGenericPassword(
        nullptr,
        static_cast<UInt32>(service.size()), service.c_str(),
        static_cast<UInt32>(account.size()), account.c_str(),
        &data_len, &data,
        &item
    );

    if (status != errSecSuccess || item == nullptr) {
        return false;
    }

    status = SecKeychainItemDelete(item);
    CFRelease(item);
    if (data) SecKeychainItemFreeContent(nullptr, data);
    return status == errSecSuccess;
}

// ---------------------------------------------------------------------------
// Introspection
// ---------------------------------------------------------------------------

bool DarwinKeychainStore::is_available() const {
    SecKeychainRef keychain = nullptr;
    OSStatus status = SecKeychainCopyDefault(&keychain);
    if (status != errSecSuccess || keychain == nullptr) {
        return false;
    }

    SecKeychainStatus keychain_status = 0;
    status = SecKeychainGetStatus(keychain, &keychain_status);
    CFRelease(keychain);

    return status == errSecSuccess &&
           (keychain_status & kSecUnlockStateStatus) != 0;
}

std::string DarwinKeychainStore::backend_name() const {
    return "macos-keychain";
}

} // namespace exv::platform::darwin

#endif // __APPLE__
