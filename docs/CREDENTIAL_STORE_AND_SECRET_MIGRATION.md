# Credential Store and Secret Migration

## Threat Model

The EXV client handles the following sensitive material:

| Secret type        | Lifetime          | Risk if leaked                         |
|--------------------|-------------------|----------------------------------------|
| VPN password       | Per-session       | Unauthorized VPN access                |
| Session cookie     | Per-connection    | Session hijack                         |
| Group auth token   | Per-auth-flow     | Credential stuffing against IdP        |

**Attack surfaces considered:**

1. **Process arguments** -- secrets must never appear in argv (covered by `no_secret_in_argv_test`).
2. **Log output** -- secrets must never appear in log lines or diagnostic snapshots (covered by `no_secret_in_logs_test`).
3. **Disk at rest** -- secrets must be encrypted by the OS credential subsystem, never stored in plaintext config files.
4. **IPC payload** -- the helper IPC protocol must not carry raw credentials in its JSON payload.

---

## CredentialStore Interface

`src/platform/common/credential_store.hpp` defines the abstract interface:

```cpp
class CredentialStore {
public:
    virtual ~CredentialStore() = default;

    virtual bool save(const std::string& profile_id,
                      const std::string& secret_name,
                      const std::string& secret) = 0;

    virtual std::optional<std::string> load(const std::string& profile_id,
                                            const std::string& secret_name) const = 0;

    virtual bool erase(const std::string& profile_id,
                       const std::string& secret_name) = 0;

    virtual bool is_available() const = 0;
    virtual std::string backend_name() const = 0;

    static std::unique_ptr<CredentialStore> create();
};
```

Key design decisions:

- **Two-part key** -- `(profile_id, secret_name)` allows per-profile isolation and multiple secrets per profile.
- **Factory pattern** -- `create()` returns the platform-appropriate implementation so callers never need `#ifdef`.
- **Structured failure** -- `save()` returns `bool`, `load()` returns `std::optional`. No exceptions for expected failure paths.

---

## Platform Implementations

### Windows: Credential Manager

**File:** `src/platform/win32/win_credential_store.hpp/.cpp`

Uses the Win32 Credential Manager API (`CredWriteW` / `CredReadW` / `CredDeleteW`).

- Target name format: `EXV/<profile_id>/<secret_name>`
- Persistence: `CRED_PERSIST_LOCAL_MACHINE` -- survives reboots, not roamed.
- Encoding: UTF-8 to wide-char conversion via `MultiByteToWideChar`.
- Encryption: handled transparently by Windows (DPAPI under the hood).

### macOS: Keychain

**File:** `src/platform/darwin/darwin_keychain_store.hpp/.cpp`

Uses the Security framework (`SecKeychainAddGenericPassword` / `SecKeychainFindGenericPassword` / `SecKeychainItemDelete`).

- Service name: `com.exv.profile.<profile_id>`
- Account name: `<profile_id>/<secret_name>`
- Encryption: handled transparently by the Keychain (AES-256 with user's keychain password).

### Linux: Unsupported (explicit)

No standard OS-level secret store is universally available on Linux (GNOME Keyring, KSecret Service, etc. vary by desktop environment). The factory returns an `UnsupportedCredentialStore` whose every method produces a clear, observable warning:

- `is_available()` returns `false`
- `backend_name()` returns `"unsupported"`
- `save()` logs a `WARNING` to stderr and returns `false`
- `load()` returns `std::nullopt`

**Future work:** integrate with `libsecret` (Secret Service API) when the D-Bus session bus is available.

---

## Migration from Old Config

Older versions of EXV stored passwords in plaintext in the config JSON. The migration path is:

1. On startup, the config manager checks for legacy plaintext credentials.
2. If found, it attempts to migrate them to the CredentialStore.
3. On success, the plaintext value is overwritten with a marker string (e.g., `__migrated_to_credential_store__`).
4. On failure (store unavailable), a warning is emitted and the plaintext value is left in place with a TODO comment.

The migration is one-way and idempotent -- re-running it when credentials are already in the store is a no-op.

---

## Failure Handling

| Scenario                          | Behaviour                                                 |
|-----------------------------------|-----------------------------------------------------------|
| Backend unavailable               | `is_available()` returns `false`; `save()` returns `false` with stderr warning |
| Secret not found                  | `load()` returns `std::nullopt`                           |
| Erase nonexistent key             | `erase()` returns `false`                                 |
| OS credential store locked        | Platform API returns error; mapped to `false` / `nullopt` |
| UTF-8 encoding error              | `to_wide()` throws `std::runtime_error` (Windows only)    |

Callers should always check the return value of `save()` and handle `std::nullopt` from `load()`.

---

## Test Coverage

| Test file                                | What it verifies                                  |
|------------------------------------------|---------------------------------------------------|
| `tests/credential_store_model_test.cpp`  | Interface contract: factory, round-trip, edge cases |
| `tests/security/no_secret_in_argv_test.cpp` | No secrets in process arguments or IPC messages  |
| `tests/security/no_secret_in_logs_test.cpp`  | No secrets in log paths, error structs, snapshots |
