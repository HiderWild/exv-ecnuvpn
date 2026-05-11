#include "config_manager.hpp"
#include "logger.hpp"
#include "utils.hpp"

#include <nlohmann/json.hpp>

#include <cerrno>
#include <cstdio>
#include <cstring>
#include <sstream>
#ifndef _WIN32
#include <unistd.h>
#endif

namespace ecnuvpn {
namespace config {

ConfigManager::ConfigManager(const std::string& config_dir)
    : config_dir_(config_dir) {
    utils::ensure_dir(config_dir_);
}

Config ConfigManager::load() {
    std::lock_guard<std::mutex> lock(mutex_);

    std::string path = utils::get_config_path();
    if (!utils::file_exists(path)) {
        config_ = Config{};
        return config_;
    }

    try {
        std::string content = utils::read_file(path);
        auto j = nlohmann::json::parse(content);
        config_ = j.get<Config>();
    } catch (const std::exception& e) {
        logger::error("ConfigManager::load parse error: " +
                      std::string(e.what()));
        config_ = Config{};
    }

    return config_;
}

void ConfigManager::save(const Config& cfg) {
    std::lock_guard<std::mutex> lock(mutex_);

    utils::ensure_dir(config_dir_);

    std::string final_path = utils::get_config_path();
    std::string tmp_path = final_path + ".tmp";

    try {
        nlohmann::json j = cfg;
        if (!utils::write_file(tmp_path, j.dump(4))) {
            logger::error("ConfigManager::save: failed to write temp file: " +
                          tmp_path);
            return;
        }

        if (::rename(tmp_path.c_str(), final_path.c_str()) != 0) {
            logger::error("ConfigManager::save: rename failed: " +
                          std::string(std::strerror(errno)));
#ifdef _WIN32
            ::_unlink(tmp_path.c_str());
#else
            ::unlink(tmp_path.c_str());
#endif
            return;
        }

        config_ = cfg;
        logger::info("Config saved: " + final_path);
    } catch (const std::exception& e) {
        logger::error("ConfigManager::save error: " +
                      std::string(e.what()));
#ifdef _WIN32
        ::_unlink(tmp_path.c_str());
#else
        ::unlink(tmp_path.c_str());
#endif
    }
}

Config ConfigManager::get() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return config_;
}

} // namespace config
} // namespace ecnuvpn
