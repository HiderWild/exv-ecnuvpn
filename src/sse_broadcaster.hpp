#pragma once

#include <atomic>
#include <condition_variable>
#include <deque>
#include <functional>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace ecnuvpn {
namespace sse {

// Escape a string for inclusion in a JSON string value
std::string escape_json(const std::string& s);

// Format a complete SSE event string
std::string format_sse_event(const std::string& event_type,
                             const std::string& data);

class SseBroadcaster {
public:
    // Callback that returns the current status as a JSON string.
    // Called periodically by the status poller thread.
    using StatusFn = std::function<std::string()>;

    SseBroadcaster(const std::string& log_path, StatusFn status_fn,
                   int max_clients = 4);
    ~SseBroadcaster();

    SseBroadcaster(const SseBroadcaster&) = delete;
    SseBroadcaster& operator=(const SseBroadcaster&) = delete;

    // Launch background threads (log watcher, status poller, heartbeat).
    void start();
    void stop();

    // Register a new SSE client. Returns client ID, or -1 if at capacity.
    int add_client();

    // Remove a client and clean up its queue.
    void remove_client(int client_id);

    // Blocking read from a client's event queue.
    // Returns the next SSE event string, or empty string on timeout/shutdown.
    std::string next_event(int client_id, int timeout_ms = 15000);

    // Push a raw SSE event string to all connected clients.
    void broadcast(const std::string& event_type, const std::string& data);

    bool is_full() const;
    int client_count() const;

private:
    void log_watcher();
    void status_poller();
    void heartbeat_loop();

    void push_to_all(const std::string& sse_data);
    void parse_and_broadcast_log_line(const std::string& line);

    struct ClientQueue {
        std::deque<std::string> events;
        std::mutex mtx;
        std::condition_variable cv;
        bool active = true;
    };

    std::string log_path_;
    StatusFn status_fn_;
    int max_clients_;

    std::atomic<bool> running_{false};

    std::thread log_thread_;
    std::thread status_thread_;
    std::thread heartbeat_thread_;

    mutable std::mutex clients_mutex_;
    std::vector<std::unique_ptr<ClientQueue>> clients_;

    static constexpr int HEARTBEAT_INTERVAL_SEC = 15;
    static constexpr int STATUS_POLL_INTERVAL_SEC = 3;
};

} // namespace sse
} // namespace ecnuvpn
