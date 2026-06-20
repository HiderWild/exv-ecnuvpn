#pragma once

#include "core/config/config.hpp"

#include <mutex>
#include <string>

namespace exv {
namespace config {

class ConfigManager {
public:
    explicit ConfigManager(const std::string& config_dir);

    Config load();
    // Returns true on success; false if the JSON could not be serialised or
    // the on-disk write failed. The error reason is written to the logger.
    bool save(const Config& cfg);
    Config get() const;

private:
    std::string config_dir_;
    Config config_;
    mutable std::mutex mutex_;
};

} // namespace config
} // namespace exv
