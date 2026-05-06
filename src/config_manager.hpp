#pragma once

#include "config.hpp"

#include <mutex>
#include <string>

namespace ecnuvpn {
namespace config {

class ConfigManager {
public:
    explicit ConfigManager(const std::string& config_dir);

    Config load();
    void save(const Config& cfg);
    Config get() const;

    const std::string& config_dir() const { return config_dir_; }

private:
    mutable std::mutex mutex_;
    Config config_;
    std::string config_dir_;
};

} // namespace config
} // namespace ecnuvpn
