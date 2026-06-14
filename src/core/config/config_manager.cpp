#include "core/config/config_manager.hpp"
#include "logger.hpp"
#include "utils.hpp"

#include <nlohmann/json.hpp>

#include <filesystem>

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

bool ConfigManager::save(const Config& cfg) {
    std::lock_guard<std::mutex> lock(mutex_);

    namespace fs = std::filesystem;
    std::error_code ec;
    fs::create_directories(fs::u8path(config_dir_), ec);

    std::string final_path = utils::get_config_path();
    std::string tmp_path = final_path + ".tmp";

    try {
        nlohmann::json j = cfg;
        if (!utils::write_file(tmp_path, j.dump(4))) {
            logger::error("ConfigManager::save: failed to write temp file: " +
                          tmp_path);
            return false;
        }

        // On Windows, rename fails if dest exists. Remove first, then rename.
        fs::remove(fs::u8path(final_path), ec);
        fs::rename(fs::u8path(tmp_path), fs::u8path(final_path), ec);
        if (ec) {
            // Fallback: copy_file + remove
            fs::copy_file(fs::u8path(tmp_path), fs::u8path(final_path),
                          fs::copy_options::overwrite_existing, ec);
            fs::remove(fs::u8path(tmp_path), ec);
            if (ec) {
                logger::error("ConfigManager::save: rename/copy failed: " +
                              ec.message());
                fs::remove(fs::u8path(tmp_path), ec);
                return false;
            }
        }

        config_ = cfg;
        logger::info("Config saved: " + final_path);
        return true;
    } catch (const std::exception& e) {
        logger::error("ConfigManager::save error: " +
                      std::string(e.what()));
        fs::remove(fs::u8path(tmp_path), ec);
        return false;
    }
}

Config ConfigManager::get() const {
    std::lock_guard<std::mutex> lock(mutex_);
    return config_;
}

} // namespace config
} // namespace ecnuvpn
