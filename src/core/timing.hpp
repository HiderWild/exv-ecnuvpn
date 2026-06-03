#pragma once
#include <chrono>
#include <string>
#include <map>

namespace exv::core {

class StageTimer {
public:
    void start(const std::string& stage_name);
    void end(const std::string& stage_name);
    std::chrono::milliseconds elapsed(const std::string& stage_name) const;
    std::chrono::milliseconds total_elapsed() const;
    std::map<std::string, std::chrono::milliseconds> all_stages() const;
    void reset();

private:
    struct Stage {
        std::chrono::steady_clock::time_point start_time;
        std::chrono::steady_clock::time_point end_time;
        bool completed = false;
    };
    std::map<std::string, Stage> stages_;
    std::chrono::steady_clock::time_point overall_start_;
};

struct ConnectTiming {
    StageTimer timer;
    static constexpr const char* HELPER_PREPARE = "helper_prepare";
    static constexpr const char* AUTH = "auth";
    static constexpr const char* CSTP_CONNECT = "cstp_connect";
    static constexpr const char* NETWORK_CONFIG = "network_config";
    static constexpr const char* PACKET_DEVICE = "packet_device";
    static constexpr const char* FIRST_PACKET = "first_packet";
};

} // namespace exv::core
