// Model test for the CredentialStore abstract interface.
//
// Exercises the CredentialStore returned by create() to verify the contract:
//   - save() / load() / erase() round-trip (on platforms with a real backend)
//   - load() for a nonexistent key returns std::nullopt
//   - erase() for a nonexistent key returns false
//   - backend_name() returns a non-empty string
//   - is_available() returns the correct value for the platform
//
// NOTE: This test does NOT use assert() because Release builds define NDEBUG
// which makes assert() a no-op.  All checks use runtime if/return instead.

#include "platform/common/credential_store.hpp"

#include <iostream>
#include <string>

namespace {

bool g_ok = true;

void expect(bool condition, const char* message) {
    if (!condition) {
        std::cerr << "EXPECT FAILED: " << message << std::endl;
        g_ok = false;
    }
}

} // namespace

int main() {
    std::cout << "credential_store_model_test:\n";

    // ---------------------------------------------------------------
    // 1. Factory returns non-null
    // ---------------------------------------------------------------
    {
        auto store = exv::platform::CredentialStore::create();
        expect(store != nullptr, "create() must never return null");
        std::cout << "  [PASS] factory returns non-null\n";
    }

    // ---------------------------------------------------------------
    // 2. backend_name() is non-empty
    // ---------------------------------------------------------------
    {
        auto store = exv::platform::CredentialStore::create();
        std::string name = store->backend_name();
        expect(!name.empty(), "backend_name() must not be empty");
        std::cout << "  [PASS] backend_name() = \"" << name << "\"\n";
    }

    // ---------------------------------------------------------------
    // 3. load() nonexistent returns nullopt
    // ---------------------------------------------------------------
    {
        auto store = exv::platform::CredentialStore::create();
        auto result = store->load("test-profile-nonexistent", "no-such-secret");
        expect(!result.has_value(), "load() for missing key must return nullopt");
        std::cout << "  [PASS] load nonexistent returns nullopt\n";
    }

    // ---------------------------------------------------------------
    // 4. erase() nonexistent returns false
    // ---------------------------------------------------------------
    {
        auto store = exv::platform::CredentialStore::create();
        bool ok = store->erase("test-profile-nonexistent", "no-such-secret");
        expect(!ok, "erase() for missing key must return false");
        std::cout << "  [PASS] erase nonexistent returns false\n";
    }

    // ---------------------------------------------------------------
    // 5. Platform-specific: is_available and backend_name
    // ---------------------------------------------------------------
    {
        auto store = exv::platform::CredentialStore::create();
#ifdef _WIN32
        expect(store->is_available(), "WinCredentialStore must be available");
        expect(store->backend_name() == "windows-credential-manager",
               "backend must be windows-credential-manager");
        std::cout << "  [PASS] win32 is_available and backend_name\n";
#elif defined(__APPLE__)
        expect(store->backend_name() == "macos-keychain",
               "backend must be macos-keychain");
        std::cout << "  [PASS] darwin backend_name; is_available="
                  << (store->is_available() ? "true" : "false") << "\n";
#else
        expect(!store->is_available(), "UnsupportedCredentialStore must not be available");
        expect(store->backend_name() == "unsupported",
               "backend must be unsupported");
        std::cout << "  [PASS] unsupported is_available and backend_name\n";
#endif
    }

    // ---------------------------------------------------------------
    // 6. Full round-trip (only on platforms with a real backend)
    // ---------------------------------------------------------------
    {
        auto store = exv::platform::CredentialStore::create();
        if (!store->is_available()) {
            std::cout << "  [SKIP] backend not available, skipping round-trip\n";
        } else {
            const std::string profile = "credential-store-test";
            const std::string secret_name = "unit-test-token";
            const std::string secret_value = "s3cret-v@lue-12345";

            // Save
            bool saved = store->save(profile, secret_name, secret_value);
            expect(saved, "save() must succeed");

            // Load
            auto loaded = store->load(profile, secret_name);
            expect(loaded.has_value(), "load() must find the saved secret");
            if (loaded.has_value()) {
                expect(*loaded == secret_value, "loaded value must match saved value");
            }

            // Erase
            bool erased = store->erase(profile, secret_name);
            expect(erased, "erase() must succeed for existing key");

            // Verify gone
            auto gone = store->load(profile, secret_name);
            expect(!gone.has_value(), "load() after erase must return nullopt");

            // Overwrite
            store->save(profile, secret_name, "first");
            store->save(profile, secret_name, "second");
            auto overwritten = store->load(profile, secret_name);
            expect(overwritten.has_value() && *overwritten == "second",
                   "overwrite must produce the latest value");
            store->erase(profile, secret_name);

            std::cout << "  [PASS] save/load/erase/overwrite round-trip\n";
        }
    }

    // ---------------------------------------------------------------
    // Done
    // ---------------------------------------------------------------
    if (g_ok) {
        std::cout << "all assertions passed\n";
    } else {
        std::cerr << "some assertions FAILED\n";
    }
    return g_ok ? 0 : 1;
}
