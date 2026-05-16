#include "config_api.hpp"
#include "crypto.hpp"
#include "logger.hpp"
#include "utils.hpp"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <cctype>
#include <sstream>

namespace ecnuvpn {
namespace config_api {

// ── CIDR validation (mirrors config.cpp) ──────────────────────────

static bool is_valid_cidr(const std::string& s) {
    if (s.empty()) return false;
    if (s.back() == '.' || s.back() == '/') return false;

    std::string ip_part = s;
    std::string::size_type slash = s.find('/');
    if (slash != std::string::npos) {
        std::string pstr = s.substr(slash + 1);
        if (pstr.empty()) return false;
        int prefix;
        try {
            prefix = std::stoi(pstr);
        } catch (...) {
            return false;
        }
        if (prefix < 0 || prefix > 32) return false;
        if (pstr.size() > 1 && pstr[0] == '0') return false;
        ip_part = s.substr(0, slash);
    }

    int octets = 0;
    std::istringstream iss(ip_part);
    std::string octet;
    while (std::getline(iss, octet, '.')) {
        if (octet.empty()) return false;
        for (char c : octet) {
            if (!std::isdigit(static_cast<unsigned char>(c))) return false;
        }
        int v;
        try {
            v = std::stoi(octet);
        } catch (...) {
            return false;
        }
        if (v < 0 || v > 255) return false;
        ++octets;
    }
    return octets == 4;
}

// ── Config field setters ──────────────────────────────────────────

std::string config_set(config::ConfigManager& mgr, const std::string& key,
                       const std::string& value) {
    Config cfg = mgr.load();

    if (key == "server") {
        cfg.server = value;
    } else if (key == "username") {
        cfg.username = value;
    } else if (key == "useragent") {
        cfg.useragent = value;
    } else if (key == "log_file") {
        cfg.log_file = value;
    } else if (key == "mtu") {
        int mtu;
        try {
            mtu = std::stoi(value);
        } catch (...) {
            return "Invalid MTU value";
        }
        if (mtu < 576 || mtu > 1500) return "MTU must be between 576 and 1500";
        cfg.mtu = mtu;
    } else if (key == "remember_password") {
        if (value == "true" || value == "1") {
            cfg.remember_password = true;
        } else if (value == "false" || value == "0") {
            cfg.remember_password = false;
            cfg.password = "";
        } else {
            return "Invalid boolean value for remember_password";
        }
    } else if (key == "disable_dtls") {
        if (value == "true" || value == "1") {
            cfg.disable_dtls = true;
        } else if (value == "false" || value == "0") {
            cfg.disable_dtls = false;
        } else {
            return "Invalid boolean value for disable_dtls";
        }
    } else if (key == "webui_enabled") {
        if (value == "true" || value == "1") {
            cfg.webui_enabled = true;
        } else if (value == "false" || value == "0") {
            cfg.webui_enabled = false;
        } else {
            return "Invalid boolean value for webui_enabled";
        }
    } else if (key == "openconnect_runtime") {
        if (value == "bundled" || value == "system" || value == "auto") {
            cfg.openconnect_runtime = value;
        } else {
            return "openconnect_runtime must be bundled, system, or auto";
        }
    } else if (key == "windows_tunnel_driver") {
        if (value == "auto" || value == "wintun" || value == "tap") {
            cfg.windows_tunnel_driver = value;
        } else {
            return "windows_tunnel_driver must be auto, wintun, or tap";
        }
    } else if (key == "windows_tap_interface") {
        cfg.windows_tap_interface = value;
    } else {
        return "Unknown config key: " + key;
    }

    if (!mgr.save(cfg)) {
        return "Failed to write config file. Check disk permissions for " +
               utils::get_config_path();
    }
    logger::info("Config key set via config_api: " + key);
    return "";
}

std::string config_set_password(config::ConfigManager& mgr,
                                const std::string& plaintext) {
    if (plaintext.empty()) {
        return "Password cannot be empty";
    }

    Config cfg = mgr.load();

    if (!cfg.remember_password) {
        cfg.remember_password = true;
    }

    // Make sure the encryption key is available before validating. The
    // desktop entrypoint already calls init_key_if_needed() but the CLI
    // can still hit this path on a clean install.
    crypto::init_key_if_needed();

    std::string ks = crypto::key_status();
    if (ks != "valid") {
        return "Encryption key is " + ks + ". Reset key first.";
    }

    std::string key = crypto::load_key();
    std::string encrypted = crypto::encrypt(plaintext, key);
    if (encrypted.empty()) {
        return "Encryption failed";
    }

    cfg.password = encrypted;
    if (!mgr.save(cfg)) {
        return "Failed to write config file. Check disk permissions for " +
               utils::get_config_path();
    }
    logger::info("Password updated via config_api (encrypted)");
    return "";
}

void config_reset(config::ConfigManager& mgr) {
    Config cfg;
    mgr.save(cfg);
    logger::info("Config reset to defaults via config_api");
}

// ── Config import ─────────────────────────────────────────────────

std::string config_import(config::ConfigManager& mgr, const std::string& json_str) {
    nlohmann::json j;
    try {
        j = nlohmann::json::parse(json_str);
    } catch (const std::exception& e) {
        std::ostringstream ss;
        ss << "Invalid JSON: " << e.what();
        return ss.str();
    }

    Config cfg = mgr.load();

    if (j.contains("server")) cfg.server = j["server"].get<std::string>();
    if (j.contains("username")) cfg.username = j["username"].get<std::string>();
    if (j.contains("mtu")) cfg.mtu = j["mtu"].get<int>();
    if (j.contains("useragent")) cfg.useragent = j["useragent"].get<std::string>();
    if (j.contains("disable_dtls")) cfg.disable_dtls = j["disable_dtls"].get<bool>();
    if (j.contains("routes")) cfg.routes = j["routes"].get<std::vector<std::string>>();
    if (j.contains("extra_args")) cfg.extra_args = j["extra_args"].get<std::vector<std::string>>();
    if (j.contains("log_file")) cfg.log_file = j["log_file"].get<std::string>();
    if (j.contains("remember_password")) cfg.remember_password = j["remember_password"].get<bool>();
    if (j.contains("openconnect_runtime")) cfg.openconnect_runtime = j["openconnect_runtime"].get<std::string>();
    if (j.contains("windows_tunnel_driver")) cfg.windows_tunnel_driver = j["windows_tunnel_driver"].get<std::string>();
    if (j.contains("windows_tap_interface")) cfg.windows_tap_interface = j["windows_tap_interface"].get<std::string>();

    if (j.contains("password")) {
        std::string pw = j["password"].get<std::string>();
        if (!pw.empty() && cfg.remember_password) {
            std::string ks = crypto::key_status();
            if (ks == "valid") {
                cfg.password = crypto::encrypt(pw, crypto::load_key());
            }
        }
    }

    mgr.save(cfg);
    logger::info("Config imported via config_api");
    return "";
}

// ── Route management ──────────────────────────────────────────────

std::string route_add(config::ConfigManager& mgr, const std::string& cidr) {
    if (!is_valid_cidr(cidr)) {
        return "Invalid CIDR format: " + cidr;
    }

    Config cfg = mgr.load();
    if (std::find(cfg.routes.begin(), cfg.routes.end(), cidr) != cfg.routes.end()) {
        return "Route already exists: " + cidr;
    }

    cfg.routes.push_back(cidr);
    if (!mgr.save(cfg)) {
        return "Failed to write config file. Check disk permissions for " +
               utils::get_config_path();
    }
    logger::info("Route added via config_api: " + cidr);
    return "";
}

std::string route_remove(config::ConfigManager& mgr, const std::string& cidr) {
    Config cfg = mgr.load();
    auto it = std::find(cfg.routes.begin(), cfg.routes.end(), cidr);
    if (it == cfg.routes.end()) {
        return "Route not found: " + cidr;
    }

    cfg.routes.erase(it);
    if (!mgr.save(cfg)) {
        return "Failed to write config file. Check disk permissions for " +
               utils::get_config_path();
    }
    logger::info("Route removed via config_api: " + cidr);
    return "";
}

void route_reset_defaults(config::ConfigManager& mgr) {
    Config cfg = mgr.load();
    cfg.routes = Config{}.routes;
    mgr.save(cfg);
    logger::info("Routes reset to defaults via config_api");
}

// ── Key management ────────────────────────────────────────────────

std::string key_status() {
    return crypto::key_status();
}

void key_reset_noninteractive() {
    std::string new_key = crypto::generate_key();
    if (!crypto::save_key(new_key)) {
        logger::error("key_reset_noninteractive: failed to save new key");
        return;
    }

    // Clear password ciphertext in config.json
    std::string cfg_path = utils::get_config_path();
    if (utils::file_exists(cfg_path)) {
        std::string content = utils::read_file(cfg_path);
        try {
            auto j = nlohmann::json::parse(content);
            j["password"] = "";
            utils::write_file(cfg_path, j.dump(4));
        } catch (...) {
        }
    }

    logger::info("Encryption key reset via config_api (non-interactive)");
}

} // namespace config_api
} // namespace ecnuvpn
