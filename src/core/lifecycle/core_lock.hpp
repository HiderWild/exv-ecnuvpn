#pragma once

#include <optional>
#include <string>

namespace exv::core::lifecycle {

class CoreInstanceLock {
public:
    CoreInstanceLock() = default;
    ~CoreInstanceLock();

    CoreInstanceLock(CoreInstanceLock&& other) noexcept;
    CoreInstanceLock& operator=(CoreInstanceLock&& other) noexcept;

    CoreInstanceLock(const CoreInstanceLock&) = delete;
    CoreInstanceLock& operator=(const CoreInstanceLock&) = delete;

    static std::optional<CoreInstanceLock> try_acquire();
    static std::optional<CoreInstanceLock> try_acquire(const std::string& state_dir);

    bool owns_lock() const noexcept;
    const std::string& lock_path() const noexcept;

private:
#ifdef _WIN32
    explicit CoreInstanceLock(std::string lock_path, void* handle);
#else
    explicit CoreInstanceLock(std::string lock_path, int fd);
#endif

    void release() noexcept;

    std::string lock_path_;
#ifdef _WIN32
    void* handle_ = nullptr;
#else
    int fd_ = -1;
#endif
};

} // namespace exv::core::lifecycle
