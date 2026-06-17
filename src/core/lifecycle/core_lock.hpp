#pragma once

#include <memory>
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
    struct Impl;

    explicit CoreInstanceLock(std::string lock_path, std::unique_ptr<Impl> impl);

    void release() noexcept;

    std::string lock_path_;
    std::unique_ptr<Impl> impl_;
};

} // namespace exv::core::lifecycle
