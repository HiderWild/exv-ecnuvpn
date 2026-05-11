#include "sse_broadcaster.hpp"
#include "logger.hpp"
#include "utils.hpp"

#include <cerrno>
#include <cstring>
#ifndef _WIN32
#include <fcntl.h>
#endif
#include <sstream>
#ifdef __APPLE__
#include <sys/event.h>
#elif defined(__linux__)
#include <sys/inotify.h>
#elif defined(_WIN32)
#include <windows.h>
#endif
#include <filesystem>
#ifndef _WIN32
#include <unistd.h>
#endif

namespace ecnuvpn {
namespace sse {

// ── JSON escaping ─────────────────────────────────────────────────

std::string escape_json(const std::string& s) {
    std::ostringstream ss;
    for (unsigned char c : s) {
        switch (c) {
        case '"':  ss << "\\\""; break;
        case '\\': ss << "\\\\"; break;
        case '\n': ss << "\\n"; break;
        case '\r': ss << "\\r"; break;
        case '\t': ss << "\\t"; break;
        default:
            if (c < 0x20) {
                ss << "\\u00" << std::hex << static_cast<int>(c);
            } else {
                ss << c;
            }
            break;
        }
    }
    return ss.str();
}

// ── SSE formatting ────────────────────────────────────────────────

std::string format_sse_event(const std::string& event_type,
                             const std::string& data) {
    std::ostringstream ss;
    ss << "event: " << event_type << "\n"
       << "data: " << data << "\n\n";
    return ss.str();
}

// ── Broadcaster ───────────────────────────────────────────────────

SseBroadcaster::SseBroadcaster(const std::string& log_path, StatusFn status_fn,
                               int max_clients)
    : log_path_(log_path), status_fn_(std::move(status_fn)),
      max_clients_(max_clients) {}

SseBroadcaster::~SseBroadcaster() { stop(); }

void SseBroadcaster::start() {
    if (running_) return;
    running_ = true;

    log_thread_ = std::thread(&SseBroadcaster::log_watcher, this);
    status_thread_ = std::thread(&SseBroadcaster::status_poller, this);
    heartbeat_thread_ = std::thread(&SseBroadcaster::heartbeat_loop, this);
}

void SseBroadcaster::stop() {
    if (!running_) return;
    running_ = false;

    // Wake all clients so their threads can exit
    {
        std::lock_guard<std::mutex> lock(clients_mutex_);
        for (auto& cq : clients_) {
            std::lock_guard<std::mutex> qlock(cq->mtx);
            cq->active = false;
            cq->cv.notify_all();
        }
    }

    if (log_thread_.joinable()) log_thread_.join();
    if (status_thread_.joinable()) status_thread_.join();
    if (heartbeat_thread_.joinable()) heartbeat_thread_.join();
}

int SseBroadcaster::add_client() {
    std::lock_guard<std::mutex> lock(clients_mutex_);
    if (static_cast<int>(clients_.size()) >= max_clients_) return -1;

    int id = static_cast<int>(clients_.size());
    clients_.push_back(std::make_unique<ClientQueue>());
    return id;
}

void SseBroadcaster::remove_client(int client_id) {
    std::lock_guard<std::mutex> lock(clients_mutex_);
    if (client_id < 0 || client_id >= static_cast<int>(clients_.size())) return;

    auto& cq = clients_[client_id];
    {
        std::lock_guard<std::mutex> qlock(cq->mtx);
        cq->active = false;
        cq->cv.notify_all();
    }
    clients_[client_id].reset();
}

std::string SseBroadcaster::next_event(int client_id, int timeout_ms) {
    ClientQueue* cq = nullptr;
    {
        std::lock_guard<std::mutex> lock(clients_mutex_);
        if (client_id < 0 || client_id >= static_cast<int>(clients_.size()))
            return "";
        cq = clients_[client_id].get();
    }
    if (!cq) return "";

    std::unique_lock<std::mutex> lock(cq->mtx);
    if (cq->events.empty() && cq->active) {
        cq->cv.wait_for(lock, std::chrono::milliseconds(timeout_ms));
    }

    if (!cq->active && cq->events.empty()) return "";

    if (cq->events.empty()) return "";

    std::string event = std::move(cq->events.front());
    cq->events.pop_front();
    return event;
}

void SseBroadcaster::broadcast(const std::string& event_type,
                               const std::string& data) {
    std::string formatted = format_sse_event(event_type, data);
    push_to_all(formatted);
}

bool SseBroadcaster::is_full() const {
    std::lock_guard<std::mutex> lock(clients_mutex_);
    return static_cast<int>(clients_.size()) >= max_clients_;
}

int SseBroadcaster::client_count() const {
    std::lock_guard<std::mutex> lock(clients_mutex_);
    int count = 0;
    for (auto& cq : clients_) {
        if (cq) ++count;
    }
    return count;
}

// ── Internal methods ──────────────────────────────────────────────

void SseBroadcaster::push_to_all(const std::string& sse_data) {
    std::lock_guard<std::mutex> lock(clients_mutex_);
    for (auto& cq : clients_) {
        if (!cq) continue;
        std::lock_guard<std::mutex> qlock(cq->mtx);
        cq->events.push_back(sse_data);
        cq->cv.notify_one();
    }
}

void SseBroadcaster::parse_and_broadcast_log_line(const std::string& line) {
    // Log format: [YYYY-MM-DD HH:MM:SS] [LEVEL] message
    if (line.size() < 23 || line[0] != '[') return;

    std::string::size_type ts_end = line.find("] [");
    if (ts_end == std::string::npos || ts_end < 2) return;

    std::string timestamp = line.substr(1, ts_end - 1);

    std::string::size_type lvl_start = ts_end + 3;
    std::string::size_type lvl_end = line.find(']', lvl_start);
    if (lvl_end == std::string::npos) return;

    std::string level = line.substr(lvl_start, lvl_end - lvl_start);

    std::string message = line.substr(lvl_end + 2);

    // Trim trailing \r
    if (!message.empty() && message.back() == '\r') message.pop_back();

    std::ostringstream json;
    json << "{\"timestamp\":\"" << escape_json(timestamp) << "\""
         << ",\"level\":\"" << escape_json(level) << "\""
         << ",\"message\":\"" << escape_json(message) << "\"}";

    push_to_all(format_sse_event("log", json.str()));
}

// ── Background threads ────────────────────────────────────────────

void SseBroadcaster::log_watcher() {
#ifdef __APPLE__
    int kq = kqueue();
    if (kq < 0) {
        logger::error("SseBroadcaster: kqueue() failed: " +
                      std::string(std::strerror(errno)));
        return;
    }

    auto register_fd = [&](int fd) -> bool {
        struct kevent change;
        EV_SET(&change, fd, EVFILT_VNODE, EV_ADD | EV_ENABLE | EV_CLEAR,
               NOTE_WRITE | NOTE_EXTEND | NOTE_DELETE,
               0, NULL);
        if (kevent(kq, &change, 1, NULL, 0, NULL) < 0) {
            logger::error("SseBroadcaster: kevent register failed: " +
                          std::string(std::strerror(errno)));
            return false;
        }
        return true;
    };

    int fd = -1;
    auto open_log = [&]() -> bool {
        if (fd >= 0) {
            close(fd);
            fd = -1;
        }
        if (!utils::file_exists(log_path_)) return false;
        fd = ::open(log_path_.c_str(), O_RDONLY);
        if (fd < 0) return false;
        // Seek to end so we only read new data
        ::lseek(fd, 0, SEEK_END);
        return register_fd(fd);
    };

    // Initial open — don't fail if file doesn't exist yet
    if (utils::file_exists(log_path_)) {
        open_log();
    }

    char buf[4096];
    std::string leftover;

    while (running_) {
        struct kevent event;
        struct timespec timeout = {1, 0}; // 1 second poll interval
        int n = kevent(kq, NULL, 0, &event, 1, &timeout);

        if (n < 0) {
            if (errno == EINTR) continue;
            logger::error("SseBroadcaster: kevent wait failed: " +
                          std::string(std::strerror(errno)));
            break;
        }

        if (n == 0) {
            // Timeout — try to open log if we don't have it yet
            if (fd < 0 && utils::file_exists(log_path_)) {
                open_log();
            }
            continue;
        }

        if (event.filter == EVFILT_VNODE) {
            if (event.fflags & NOTE_DELETE) {
                leftover.clear();
                open_log();
                continue;
            }

            if (event.fflags & (NOTE_WRITE | NOTE_EXTEND)) {
                if (fd < 0) {
                    if (!open_log()) continue;
                }

                ssize_t nread;
                while ((nread = ::read(fd, buf, sizeof(buf))) > 0) {
                    leftover.append(buf, static_cast<size_t>(nread));

                    std::string::size_type pos;
                    while ((pos = leftover.find('\n')) != std::string::npos) {
                        std::string line = leftover.substr(0, pos);
                        leftover.erase(0, pos + 1);
                        if (!line.empty()) {
                            parse_and_broadcast_log_line(line);
                        }
                    }
                }
            }
        }
    }

    if (fd >= 0) close(fd);
    close(kq);
#elif defined(__linux__)
    // Linux: inotify-based file watching
    int inotify_fd = inotify_init1(IN_NONBLOCK);
    if (inotify_fd < 0) {
        logger::error("SseBroadcaster: inotify_init1() failed: " +
                      std::string(std::strerror(errno)));
        return;
    }

    int watch_fd = -1;
    int fd = -1;

    auto add_watch = [&]() -> bool {
        if (watch_fd >= 0) {
            inotify_rm_watch(inotify_fd, watch_fd);
            watch_fd = -1;
        }
        if (!utils::file_exists(log_path_)) return false;
        watch_fd = inotify_add_watch(inotify_fd, log_path_.c_str(),
                                      IN_MODIFY | IN_DELETE_SELF | IN_MOVE_SELF);
        return watch_fd >= 0;
    };

    auto open_log = [&]() -> bool {
        if (fd >= 0) {
            close(fd);
            fd = -1;
        }
        if (!utils::file_exists(log_path_)) return false;
        fd = ::open(log_path_.c_str(), O_RDONLY);
        if (fd < 0) return false;
        ::lseek(fd, 0, SEEK_END);
        return true;
    };

    // Initial setup
    if (utils::file_exists(log_path_)) {
        open_log();
        add_watch();
    }

    char buf[4096];
    // inotify events need aligned buffer
    alignas(struct inotify_event) char inotify_buf[4096];
    std::string leftover;

    while (running_) {
        // Poll with 1-second timeout using select
        struct timeval tv = {1, 0};
        fd_set readfds;
        FD_ZERO(&readfds);
        FD_SET(inotify_fd, &readfds);
        int n = select(inotify_fd + 1, &readfds, nullptr, nullptr, &tv);

        if (n < 0) {
            if (errno == EINTR) continue;
            logger::error("SseBroadcaster: select() failed: " +
                          std::string(std::strerror(errno)));
            break;
        }

        if (n == 0) {
            // Timeout — try to set up watch if file appeared
            if (watch_fd < 0 && utils::file_exists(log_path_)) {
                add_watch();
                open_log();
            }
            continue;
        }

        // Read inotify events
        ssize_t len = read(inotify_fd, inotify_buf, sizeof(inotify_buf));
        if (len < 0) {
            if (errno == EINTR) continue;
            continue;
        }

        bool file_modified = false;
        bool file_deleted = false;
        for (char *ptr = inotify_buf; ptr < inotify_buf + len; ) {
            auto *event = reinterpret_cast<struct inotify_event *>(ptr);
            if (event->mask & IN_MODIFY) file_modified = true;
            if (event->mask & (IN_DELETE_SELF | IN_MOVE_SELF)) file_deleted = true;
            ptr += sizeof(struct inotify_event) + event->len;
        }

        if (file_deleted) {
            leftover.clear();
            inotify_rm_watch(inotify_fd, watch_fd);
            watch_fd = -1;
            if (fd >= 0) { close(fd); fd = -1; }
            // Re-add when file reappears
            if (utils::file_exists(log_path_)) {
                add_watch();
                open_log();
            }
            continue;
        }

        if (file_modified) {
            if (fd < 0) {
                if (!open_log()) continue;
            }

            ssize_t nread;
            while ((nread = ::read(fd, buf, sizeof(buf))) > 0) {
                leftover.append(buf, static_cast<size_t>(nread));

                std::string::size_type pos;
                while ((pos = leftover.find('\n')) != std::string::npos) {
                    std::string line = leftover.substr(0, pos);
                    leftover.erase(0, pos + 1);
                    if (!line.empty()) {
                        parse_and_broadcast_log_line(line);
                    }
                }
            }
        }
    }

    if (fd >= 0) close(fd);
    if (watch_fd >= 0) inotify_rm_watch(inotify_fd, watch_fd);
    close(inotify_fd);
#elif defined(_WIN32)
    // Windows: ReadDirectoryChangesW-based file watching
    std::string dir_path = std::filesystem::path(log_path_).parent_path().string();
    HANDLE hDir = CreateFileA(
        dir_path.c_str(),
        FILE_LIST_DIRECTORY,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
        NULL, OPEN_EXISTING,
        FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OVERLAPPED,
        NULL);

    if (hDir == INVALID_HANDLE_VALUE) {
        logger::error("SseBroadcaster: CreateFile failed for directory: " + dir_path);
        return;
    }

    OVERLAPPED overlapped = {};
    overlapped.hEvent = CreateEvent(NULL, TRUE, FALSE, NULL);

    HANDLE hFile = INVALID_HANDLE_VALUE;
    auto open_log = [&]() -> bool {
        if (hFile != INVALID_HANDLE_VALUE) {
            CloseHandle(hFile);
            hFile = INVALID_HANDLE_VALUE;
        }
        if (!utils::file_exists(log_path_)) return false;
        hFile = CreateFileA(log_path_.c_str(), GENERIC_READ, FILE_SHARE_READ | FILE_SHARE_WRITE,
                            NULL, OPEN_EXISTING, FILE_ATTRIBUTE_NORMAL, NULL);
        if (hFile == INVALID_HANDLE_VALUE) return false;
        // Seek to end so we only read new data
        SetFilePointer(hFile, 0, NULL, FILE_END);
        return true;
    };

    if (utils::file_exists(log_path_)) {
        open_log();
    }

    alignas(DWORD) char notifyBuf[4096];
    char buf[4096];
    std::string leftover;

    while (running_) {
        ZeroMemory(notifyBuf, sizeof(notifyBuf));
        DWORD bytesReturned = 0;
        ReadDirectoryChangesW(hDir, notifyBuf, sizeof(notifyBuf), FALSE,
                              FILE_NOTIFY_CHANGE_LAST_WRITE | FILE_NOTIFY_CHANGE_FILE_NAME,
                              &bytesReturned, &overlapped, NULL);

        DWORD waitResult = WaitForSingleObject(overlapped.hEvent, 1000);
        if (waitResult == WAIT_TIMEOUT) {
            if (hFile == INVALID_HANDLE_VALUE && utils::file_exists(log_path_)) {
                open_log();
            }
            continue;
        }
        if (waitResult != WAIT_OBJECT_0) continue;

        if (!GetOverlappedResult(hDir, &overlapped, &bytesReturned, FALSE)) {
            ResetEvent(overlapped.hEvent);
            continue;
        }

        FILE_NOTIFY_INFORMATION *pNotify = reinterpret_cast<FILE_NOTIFY_INFORMATION *>(notifyBuf);
        bool modified = false;
        while (true) {
            if (pNotify->Action == FILE_ACTION_MODIFIED ||
                pNotify->Action == FILE_ACTION_RENAMED_NEW_NAME) {
                modified = true;
            }
            if (pNotify->NextEntryOffset == 0) break;
            pNotify = reinterpret_cast<FILE_NOTIFY_INFORMATION *>(
                reinterpret_cast<char *>(pNotify) + pNotify->NextEntryOffset);
        }

        if (modified) {
            if (hFile == INVALID_HANDLE_VALUE) {
                if (!open_log()) { ResetEvent(overlapped.hEvent); continue; }
            }
            DWORD bytesRead = 0;
            while (ReadFile(hFile, buf, sizeof(buf) - 1, &bytesRead, NULL) && bytesRead > 0) {
                leftover.append(buf, bytesRead);
                std::string::size_type pos;
                while ((pos = leftover.find('\n')) != std::string::npos) {
                    std::string line = leftover.substr(0, pos);
                    leftover.erase(0, pos + 1);
                    if (!line.empty()) parse_and_broadcast_log_line(line);
                }
            }
        }
        ResetEvent(overlapped.hEvent);
    }

    if (hFile != INVALID_HANDLE_VALUE) CloseHandle(hFile);
    CloseHandle(overlapped.hEvent);
    CloseHandle(hDir);
#endif
}

void SseBroadcaster::status_poller() {
    while (running_) {
        for (int i = 0; i < STATUS_POLL_INTERVAL_SEC && running_; ++i) {
            std::this_thread::sleep_for(std::chrono::seconds(1));
        }
        if (!running_) break;

        if (!status_fn_) continue;

        std::string status_json;
        try {
            status_json = status_fn_();
        } catch (...) {
            continue;
        }

        if (!status_json.empty()) {
            push_to_all(format_sse_event("status", status_json));
        }
    }
}

void SseBroadcaster::heartbeat_loop() {
    while (running_) {
        for (int i = 0; i < HEARTBEAT_INTERVAL_SEC && running_; ++i) {
            std::this_thread::sleep_for(std::chrono::seconds(1));
        }
        if (!running_) break;

        push_to_all(": heartbeat\n\n");
    }
}

} // namespace sse
} // namespace ecnuvpn
