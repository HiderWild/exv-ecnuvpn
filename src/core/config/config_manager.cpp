#include "core/config/config_initialization.hpp"
#include "core/config/config_manager.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "platform/common/path_utils.hpp"
#include "platform/common/process_utils.hpp"

#include <nlohmann/json.hpp>

#include <filesystem>

namespace exv {
namespace config {

ConfigManager::ConfigManager(const std::string& config_dir)
    : config_dir_(config_dir) {
    platform::ensure_dir(config_dir_);
}

Config ConfigManager::load() {
    std::lock_guard<std::mutex> lock(mutex_);

    const auto initialized = ensure_initialized_config(config_dir_);
    config_ = initialized.config;
    normalize_native_only(config_);
    return config_;
}

bool ConfigManager::save(const Config& cfg) {
    std::lock_guard<std::mutex> lock(mutex_);

    namespace fs = std::filesystem;
    std::error_code ec;
    fs::create_directories(fs::u8path(config_dir_), ec);

    std::string final_path = platform::config_path(config_dir_);
    std::string tmp_path = final_path + ".tmp";

    try {
        Config normalized = cfg;
        normalize_native_only(normalized);
        nlohmann::json j = normalized;
        if (!platform::write_file(tmp_path, j.dump(4))) {
            exv::observability::LogFacade::error("ConfigManager::save: failed to write temp file: " +
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
                exv::observability::LogFacade::error("ConfigManager::save: rename/copy failed: " +
                              ec.message());
                fs::remove(fs::u8path(tmp_path), ec);
                return false;
            }
        }

        config_ = normalized;
        exv::observability::LogFacade::info("Config saved: " + final_path);
        return true;
    } catch (const std::exception& e) {
        exv::observability::LogFacade::error("ConfigManager::save error: " +
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
} // namespace exv
