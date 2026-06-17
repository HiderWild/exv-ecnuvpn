#include "core/lifecycle/core_lock.hpp"

#include "core/lifecycle/core_paths.hpp"
#include "runtime/runtime_context.hpp"

#include <cstdint>
#include <filesystem>
#include <utility>

#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <fcntl.h>
#include <sys/file.h>
#include <unistd.h>
#endif

namespace exv::core::lifecycle {
namespace {

#ifdef _WIN32
std::string mutex_name_for_lock_path(const std::string& lock_path) {
    std::uint64_t hash = 1469598103934665603ull;
    for (unsigned char ch : lock_path) {
        hash ^= static_cast<std::uint64_t>(ch);
        hash *= 1099511628211ull;
    }
    return "Local\\ecnuvpn-core-lock-" + std::to_string(hash);
}
#endif

} // namespace

struct CoreInstanceLock::Impl {
#ifdef _WIN32
    HANDLE handle = nullptr;
    std::string lock_path;

    ~Impl() {
        if (handle != nullptr) {
            ReleaseMutex(handle);
            CloseHandle(handle);
            handle = nullptr;
        }
    }
#else
    int fd = -1;
    std::string lock_path;

    ~Impl() {
        if (fd >= 0) {
            flock(fd, LOCK_UN);
            close(fd);
            fd = -1;
            std::error_code ec;
            std::filesystem::remove(lock_path, ec);
        }
    }
#endif
};

CoreInstanceLock::CoreInstanceLock(std::string lock_path, std::unique_ptr<Impl> impl)
    : lock_path_(std::move(lock_path)), impl_(std::move(impl)) {}

CoreInstanceLock::~CoreInstanceLock() { release(); }

CoreInstanceLock::CoreInstanceLock(CoreInstanceLock&& other) noexcept
    : lock_path_(std::move(other.lock_path_)), impl_(std::move(other.impl_)) {}

CoreInstanceLock& CoreInstanceLock::operator=(CoreInstanceLock&& other) noexcept {
    if (this == &other) {
        return *this;
    }

    release();
    lock_path_ = std::move(other.lock_path_);
    impl_ = std::move(other.impl_);
    return *this;
}

std::optional<CoreInstanceLock> CoreInstanceLock::try_acquire() {
    return try_acquire(ecnuvpn::runtime::paths().state_dir);
}

std::optional<CoreInstanceLock> CoreInstanceLock::try_acquire(
    const std::string& state_dir) {
    const auto lock_path = core_lock_path(state_dir);

#ifdef _WIN32
    const auto mutex_name = mutex_name_for_lock_path(lock_path);
    HANDLE handle = CreateMutexA(nullptr, TRUE, mutex_name.c_str());
    if (handle == nullptr) {
        return std::nullopt;
    }
    if (GetLastError() == ERROR_ALREADY_EXISTS) {
        CloseHandle(handle);
        return std::nullopt;
    }
    auto impl = std::make_unique<Impl>();
    impl->handle = handle;
    impl->lock_path = lock_path;
    return CoreInstanceLock(lock_path, std::move(impl));
#else
    std::error_code ec;
    std::filesystem::create_directories(std::filesystem::path(lock_path).parent_path(), ec);
    int fd = open(lock_path.c_str(), O_CREAT | O_RDWR, 0600);
    if (fd < 0) {
        return std::nullopt;
    }
    if (flock(fd, LOCK_EX | LOCK_NB) != 0) {
        close(fd);
        return std::nullopt;
    }
    auto impl = std::make_unique<Impl>();
    impl->fd = fd;
    impl->lock_path = lock_path;
    return CoreInstanceLock(lock_path, std::move(impl));
#endif
}

bool CoreInstanceLock::owns_lock() const noexcept {
    return impl_ != nullptr;
}

const std::string& CoreInstanceLock::lock_path() const noexcept {
    return lock_path_;
}

void CoreInstanceLock::release() noexcept {
    impl_.reset();
}

} // namespace exv::core::lifecycle
