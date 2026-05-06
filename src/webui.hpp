#pragma once

#include "config_manager.hpp"
#include "sse_broadcaster.hpp"

#include <atomic>
#include <future>
#include <memory>
#include <mutex>
#include <string>
#include <thread>
#include <vector>

namespace httplib {
class Server;
struct Request;
struct Response;
} // namespace httplib

namespace ecnuvpn {
namespace webui {

class WebUIServer {
public:
    WebUIServer(config::ConfigManager& config_mgr,
                sse::SseBroadcaster& log_broadcaster,
                sse::SseBroadcaster& status_broadcaster,
                int port, const std::string& bind_address);
    ~WebUIServer();

    WebUIServer(const WebUIServer&) = delete;
    WebUIServer& operator=(const WebUIServer&) = delete;

    void start();
    void stop();
    bool is_running() const;

private:
    void setup_routes();

    config::ConfigManager& config_mgr_;
    sse::SseBroadcaster& log_broadcaster_;
    sse::SseBroadcaster& status_broadcaster_;
    int port_;
    std::string bind_address_;

    std::atomic<bool> vpn_connecting_{false};
    std::atomic<bool> vpn_disconnecting_{false};

    std::unique_ptr<httplib::Server> server_;
    std::thread server_thread_;
    std::atomic<bool> running_{false};

    mutable std::mutex tasks_mutex_;
    std::vector<std::future<void>> async_tasks_;
};

} // namespace webui
} // namespace ecnuvpn
