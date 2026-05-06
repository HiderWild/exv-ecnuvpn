#include "webui.hpp"

#include "config_api.hpp"
#include "crypto.hpp"
#include "helper.hpp"
#include "utils.hpp"

#include <nlohmann/json.hpp>
#include <httplib.h>

#include <sys/socket.h>
#include <sys/un.h>

#include <cerrno>
#include <cstring>
#include <fstream>
#include <sstream>
#include <unistd.h>

namespace ecnuvpn {
namespace webui {

namespace {

constexpr const char* kStaticDir = "webui/dist";
constexpr const char* kHelperSocketPath = "/var/run/exv-helper.sock";

std::string mime_type(const std::string& path) {
    if (path.size() >= 5 && path.compare(path.size() - 5, 5, ".html") == 0)
        return "text/html";
    if (path.size() >= 4 && path.compare(path.size() - 4, 4, ".css") == 0)
        return "text/css";
    if (path.size() >= 3 && path.compare(path.size() - 3, 3, ".js") == 0)
        return "application/javascript";
    if (path.size() >= 5 && path.compare(path.size() - 5, 5, ".json") == 0)
        return "application/json";
    if (path.size() >= 4 && path.compare(path.size() - 4, 4, ".png") == 0)
        return "image/png";
    if (path.size() >= 4 && path.compare(path.size() - 4, 4, ".svg") == 0)
        return "image/svg+xml";
    if (path.size() >= 4 && path.compare(path.size() - 4, 4, ".ico") == 0)
        return "image/x-icon";
    if (path.size() >= 5 && path.compare(path.size() - 5, 5, ".woff") == 0)
        return "font/woff";
    if (path.size() >= 6 && path.compare(path.size() - 6, 6, ".woff2") == 0)
        return "font/woff2";
    return "application/octet-stream";
}

bool read_file_binary(const std::string& path, std::string& out) {
    std::ifstream ifs(path, std::ios::binary | std::ios::ate);
    if (!ifs.is_open()) return false;
    auto size = ifs.tellg();
    if (size <= 0) return false;
    ifs.seekg(0);
    out.resize(static_cast<size_t>(size));
    ifs.read(&out[0], size);
    return ifs.good();
}

void serve_static(const httplib::Request& req, httplib::Response& res) {
    std::string req_path = req.path;
    if (req_path.empty() || req_path == "/") req_path = "/index.html";

    std::string file_path = std::string(kStaticDir) + req_path;

    std::string content;
    if (read_file_binary(file_path, content)) {
        res.set_content(content, mime_type(file_path));
        res.set_header("X-Content-Type-Options", "nosniff");
        return;
    }

    // SPA fallback: serve index.html for non-file paths
    std::string index_path = std::string(kStaticDir) + "/index.html";
    if (read_file_binary(index_path, content)) {
        res.set_content(content, "text/html");
        res.set_header("X-Content-Type-Options", "nosniff");
        return;
    }

    res.status = 404;
    res.set_content("{\"error\":\"not found\"}", "application/json");
}

nlohmann::json send_helper_request(const nlohmann::json& request) {
    int fd = socket(AF_UNIX, SOCK_STREAM, 0);
    if (fd < 0) {
        return nlohmann::json{{"ok", false},
                              {"message", "Failed to create socket"}};
    }

    sockaddr_un addr{};
    addr.sun_family = AF_UNIX;
    std::snprintf(addr.sun_path, sizeof(addr.sun_path), "%s",
                  kHelperSocketPath);

    if (connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
        close(fd);
        return nlohmann::json{{"ok", false},
                              {"message", "Helper daemon not available"}};
    }

    std::string payload = request.dump();
    payload.push_back('\n');
    if (write(fd, payload.data(), payload.size()) !=
        static_cast<ssize_t>(payload.size())) {
        close(fd);
        return nlohmann::json{{"ok", false},
                              {"message", "Failed to send helper request"}};
    }
    shutdown(fd, SHUT_WR);

    std::string raw;
    char buffer[1024];
    ssize_t n;
    while ((n = read(fd, buffer, sizeof(buffer))) > 0) {
        raw.append(buffer, static_cast<size_t>(n));
        if (raw.find('\n') != std::string::npos) break;
    }
    close(fd);

    auto nl = raw.find('\n');
    if (nl != std::string::npos) raw.resize(nl);
    raw = utils::trim(raw);

    if (raw.empty()) {
        return nlohmann::json{{"ok", false},
                              {"message", "Empty helper response"}};
    }

    try {
        return nlohmann::json::parse(raw);
    } catch (...) {
        return nlohmann::json{{"ok", false},
                              {"message", "Failed to parse helper response"}};
    }
}

// Build a frontend-friendly status response from helper daemon response
nlohmann::json build_frontend_status(const nlohmann::json& helper_resp,
                                      const Config& cfg) {
    nlohmann::json j;
    bool running = helper_resp.value("running", false);
    j["connected"] = running;
    j["server"] = helper_resp.value("server", cfg.server);
    j["username"] = cfg.username;
    j["pid"] = helper_resp.value("pid", -1);
    j["supervisor_pid"] = helper_resp.value("supervisor_pid", -1);
    j["network_ready"] = helper_resp.value("network_ready", false);
    j["interface"] = helper_resp.value("interface", "");
    j["internal_ip"] = helper_resp.value("internal_ip", "");
    j["route_count"] = helper_resp.value("route_count", 0);
    j["mtu"] = cfg.mtu;
    // These fields are not easily available without live traffic stats
    j["uptime_seconds"] = 0;
    j["rx_bytes"] = 0;
    j["tx_bytes"] = 0;
    return j;
}

// Build auth config subset with frontend-friendly field names
nlohmann::json build_auth_config(const Config& cfg) {
    nlohmann::json j;
    j["server"] = cfg.server;
    j["username"] = cfg.username;
    j["password"] = cfg.password.empty() ? "" : "\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2";
    j["user_agent"] = cfg.useragent;
    j["remember_password"] = cfg.remember_password;
    return j;
}

// Build settings config subset with frontend-friendly field names
nlohmann::json build_settings_config(const Config& cfg) {
    nlohmann::json j;
    j["mtu"] = cfg.mtu;
    j["dtls"] = !cfg.disable_dtls;
    // Join extra_args vector into a single string for the frontend
    std::string extra_str;
    for (size_t i = 0; i < cfg.extra_args.size(); ++i) {
        if (i > 0) extra_str += " ";
        extra_str += cfg.extra_args[i];
    }
    j["extra_args"] = extra_str;
    j["log_path"] = cfg.log_file;
    j["webui_port"] = cfg.webui_port;
    j["webui_host"] = cfg.webui_bind;
    j["webui_enabled"] = cfg.webui_enabled;
    return j;
}

} // namespace

// ── Constructor / Destructor ────────────────────────────────────────

WebUIServer::WebUIServer(config::ConfigManager& config_mgr,
                         sse::SseBroadcaster& log_broadcaster,
                         sse::SseBroadcaster& status_broadcaster,
                         int port, const std::string& bind_address)
    : config_mgr_(config_mgr),
      log_broadcaster_(log_broadcaster),
      status_broadcaster_(status_broadcaster),
      port_(port),
      bind_address_(bind_address) {
    server_ = std::make_unique<httplib::Server>();
    setup_routes();
}

WebUIServer::~WebUIServer() { stop(); }

// ── Route setup ─────────────────────────────────────────────────────

void WebUIServer::setup_routes() {
    // ── GET /api/status ─────────────────────────────────────────────
    server_->Get("/api/status", [this](const httplib::Request&,
                                        httplib::Response& res) {
        Config cfg = config_mgr_.load();
        auto helper_resp = send_helper_request({{"action", "status"}});

        if (!helper_resp.value("ok", false) &&
            helper_resp.value("message", "") == "Helper daemon not available") {
            // Helper not available — return disconnected status with config info
            nlohmann::json j;
            j["connected"] = false;
            j["server"] = cfg.server;
            j["username"] = cfg.username;
            j["pid"] = -1;
            j["supervisor_pid"] = -1;
            j["network_ready"] = false;
            j["interface"] = "";
            j["internal_ip"] = "";
            j["route_count"] = static_cast<int>(cfg.routes.size());
            j["mtu"] = cfg.mtu;
            j["uptime_seconds"] = 0;
            j["rx_bytes"] = 0;
            j["tx_bytes"] = 0;
            res.set_content(j.dump(), "application/json");
            return;
        }

        res.set_content(build_frontend_status(helper_resp, cfg).dump(),
                        "application/json");
    });

    // ── POST /api/connect ───────────────────────────────────────────
    server_->Post("/api/connect", [this](const httplib::Request& req,
                                          httplib::Response& res) {
        if (vpn_connecting_.exchange(true)) {
            res.status = 409;
            res.set_content(
                "{\"error\":\"A connect operation is already in progress\"}",
                "application/json");
            return;
        }

        std::string password;
        if (!req.body.empty()) {
            try {
                auto body = nlohmann::json::parse(req.body);
                password = body.value("password", "");
            } catch (...) {
                vpn_connecting_ = false;
                res.status = 400;
                res.set_content("{\"error\":\"invalid JSON body\"}",
                                "application/json");
                return;
            }
        }

        Config cfg = config_mgr_.load();
        if (password.empty() && !cfg.password.empty()) {
            std::string key = crypto::load_key();
            if (!key.empty()) {
                password = crypto::decrypt(cfg.password, key);
            }
        }

        auto task = std::make_shared<std::packaged_task<void()>>(
            [this, cfg, password]() {
                helper::start_via_helper(cfg, password, 0);
                vpn_connecting_ = false;
            });

        std::thread([task]() { (*task)(); }).detach();

        res.status = 202;
        res.set_content("{\"status\":\"connecting\"}", "application/json");
    });

    // ── POST /api/disconnect ────────────────────────────────────────
    server_->Post("/api/disconnect", [this](const httplib::Request&,
                                             httplib::Response& res) {
        if (vpn_disconnecting_.exchange(true)) {
            res.status = 409;
            res.set_content(
                "{\"error\":\"A disconnect operation is already in progress\"}",
                "application/json");
            return;
        }

        auto task = std::make_shared<std::packaged_task<void()>>(
            [this]() {
                helper::stop_via_helper();
                vpn_disconnecting_ = false;
            });

        std::thread([task]() { (*task)(); }).detach();

        res.status = 202;
        res.set_content("{\"status\":\"disconnecting\"}", "application/json");
    });

    // ── GET /api/config ─────────────────────────────────────────────
    server_->Get("/api/config", [this](const httplib::Request&,
                                        httplib::Response& res) {
        Config cfg = config_mgr_.load();
        nlohmann::json j = cfg;

        // Mask password
        if (!cfg.password.empty()) {
            j["password"] = "\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2";
        }

        res.set_content(j.dump(), "application/json");
    });

    // ── GET /api/config/auth ────────────────────────────────────────
    server_->Get("/api/config/auth", [this](const httplib::Request&,
                                             httplib::Response& res) {
        Config cfg = config_mgr_.load();
        res.set_content(build_auth_config(cfg).dump(), "application/json");
    });

    // ── PUT /api/config/auth ────────────────────────────────────────
    server_->Put("/api/config/auth", [this](const httplib::Request& req,
                                              httplib::Response& res) {
        nlohmann::json body;
        try {
            body = nlohmann::json::parse(req.body);
        } catch (...) {
            res.status = 400;
            res.set_content("{\"error\":\"invalid JSON body\"}",
                            "application/json");
            return;
        }

        const std::string masked = "\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2";

        // Map frontend field names to config keys
        if (body.contains("server") && body["server"].is_string()) {
            config_api::config_set(config_mgr_, "server", body["server"].get<std::string>());
        }
        if (body.contains("username") && body["username"].is_string()) {
            config_api::config_set(config_mgr_, "username", body["username"].get<std::string>());
        }
        if (body.contains("password") && body["password"].is_string()) {
            std::string pw = body["password"].get<std::string>();
            if (pw != masked && !pw.empty()) {
                config_api::config_set_password(config_mgr_, pw);
            }
        }
        if (body.contains("user_agent") && body["user_agent"].is_string()) {
            config_api::config_set(config_mgr_, "useragent", body["user_agent"].get<std::string>());
        }
        if (body.contains("remember_password") && body["remember_password"].is_boolean()) {
            config_api::config_set(config_mgr_, "remember_password",
                                   body["remember_password"].get<bool>() ? "true" : "false");
        }

        Config updated = config_mgr_.load();
        res.set_content(build_auth_config(updated).dump(), "application/json");
    });

    // ── GET /api/config/settings ────────────────────────────────────
    server_->Get("/api/config/settings", [this](const httplib::Request&,
                                                  httplib::Response& res) {
        Config cfg = config_mgr_.load();
        res.set_content(build_settings_config(cfg).dump(), "application/json");
    });

    // ── PUT /api/config/settings ─────────────────────────────────────
    server_->Put("/api/config/settings", [this](const httplib::Request& req,
                                                  httplib::Response& res) {
        nlohmann::json body;
        try {
            body = nlohmann::json::parse(req.body);
        } catch (...) {
            res.status = 400;
            res.set_content("{\"error\":\"invalid JSON body\"}",
                            "application/json");
            return;
        }

        // Map frontend field names to config keys
        if (body.contains("mtu") && body["mtu"].is_number()) {
            config_api::config_set(config_mgr_, "mtu", std::to_string(body["mtu"].get<int>()));
        }
        if (body.contains("dtls") && body["dtls"].is_boolean()) {
            // Frontend "dtls" = true means disable_dtls = false
            config_api::config_set(config_mgr_, "disable_dtls",
                                   body["dtls"].get<bool>() ? "false" : "true");
        }
        if (body.contains("extra_args") && body["extra_args"].is_string()) {
            // Store as single-element extra_args vector (space-separated split would be fragile)
            // For now, store the raw string as a single extra_arg
            Config cfg = config_mgr_.load();
            std::string val = body["extra_args"].get<std::string>();
            if (val.empty()) {
                cfg.extra_args.clear();
            } else {
                cfg.extra_args = {val};
            }
            config_mgr_.save(cfg);
        }
        if (body.contains("log_path") && body["log_path"].is_string()) {
            config_api::config_set(config_mgr_, "log_file", body["log_path"].get<std::string>());
        }
        if (body.contains("webui_port") && body["webui_port"].is_number()) {
            Config cfg = config_mgr_.load();
            cfg.webui_port = body["webui_port"].get<int>();
            config_mgr_.save(cfg);
        }
        if (body.contains("webui_host") && body["webui_host"].is_string()) {
            Config cfg = config_mgr_.load();
            cfg.webui_bind = body["webui_host"].get<std::string>();
            config_mgr_.save(cfg);
        }
        if (body.contains("webui_enabled") && body["webui_enabled"].is_boolean()) {
            config_api::config_set(config_mgr_, "webui_enabled",
                                   body["webui_enabled"].get<bool>() ? "true" : "false");
        }

        Config updated = config_mgr_.load();
        res.set_content(build_settings_config(updated).dump(), "application/json");
    });

    // ── PUT /api/config ─────────────────────────────────────────────
    server_->Put("/api/config", [this](const httplib::Request& req,
                                        httplib::Response& res) {
        nlohmann::json body;
        try {
            body = nlohmann::json::parse(req.body);
        } catch (...) {
            res.status = 400;
            res.set_content("{\"error\":\"invalid JSON body\"}",
                            "application/json");
            return;
        }

        const std::string masked = "\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2";

        for (auto it = body.begin(); it != body.end(); ++it) {
            std::string key = it.key();
            if (key == "password") {
                std::string pw = it.value().get<std::string>();
                if (pw != masked && !pw.empty()) {
                    std::string err =
                        config_api::config_set_password(config_mgr_, pw);
                    if (!err.empty()) {
                        res.status = 400;
                        nlohmann::json err_resp;
                        err_resp["error"] = err;
                        res.set_content(err_resp.dump(), "application/json");
                        return;
                    }
                }
            } else if (it.value().is_string()) {
                std::string err = config_api::config_set(
                    config_mgr_, key, it.value().get<std::string>());
                if (!err.empty()) {
                    res.status = 400;
                    nlohmann::json err_resp;
                    err_resp["error"] = err;
                    res.set_content(err_resp.dump(), "application/json");
                    return;
                }
            } else if (it.value().is_boolean()) {
                std::string err = config_api::config_set(
                    config_mgr_, key,
                    it.value().get<bool>() ? "true" : "false");
                if (!err.empty()) {
                    res.status = 400;
                    nlohmann::json err_resp;
                    err_resp["error"] = err;
                    res.set_content(err_resp.dump(), "application/json");
                    return;
                }
            } else if (it.value().is_number()) {
                std::ostringstream ss;
                ss << it.value().get<int>();
                std::string err =
                    config_api::config_set(config_mgr_, key, ss.str());
                if (!err.empty()) {
                    res.status = 400;
                    nlohmann::json err_resp;
                    err_resp["error"] = err;
                    res.set_content(err_resp.dump(), "application/json");
                    return;
                }
            }
        }

        Config updated = config_mgr_.load();
        nlohmann::json j = updated;
        if (!updated.password.empty()) {
            j["password"] = masked;
        }
        res.set_content(j.dump(), "application/json");
    });

    // ── POST /api/config/import ─────────────────────────────────────
    server_->Post("/api/config/import", [this](const httplib::Request& req,
                                                httplib::Response& res) {
        std::string err = config_api::config_import(config_mgr_, req.body);
        if (!err.empty()) {
            res.status = 400;
            nlohmann::json err_resp;
            err_resp["error"] = err;
            res.set_content(err_resp.dump(), "application/json");
            return;
        }

        Config cfg = config_mgr_.load();
        nlohmann::json j = cfg;
        if (!cfg.password.empty()) {
            j["password"] = "\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2\xe2\x80\xa2";
        }
        res.set_content(j.dump(), "application/json");
    });

    // ── POST /api/config/reset ──────────────────────────────────────
    server_->Post("/api/config/reset", [this](const httplib::Request&,
                                               httplib::Response& res) {
        config_api::config_reset(config_mgr_);
        Config cfg = config_mgr_.load();
        res.set_content(nlohmann::json(cfg).dump(), "application/json");
    });

    // ── GET /api/routes ─────────────────────────────────────────────
    server_->Get("/api/routes", [this](const httplib::Request&,
                                        httplib::Response& res) {
        Config cfg = config_mgr_.load();
        nlohmann::json arr = nlohmann::json::array();
        for (const auto& r : cfg.routes) {
            arr.push_back({{"cidr", r}});
        }
        res.set_content(arr.dump(), "application/json");
    });

    // ── POST /api/routes ────────────────────────────────────────────
    server_->Post("/api/routes", [this](const httplib::Request& req,
                                         httplib::Response& res) {
        nlohmann::json body;
        try {
            body = nlohmann::json::parse(req.body);
        } catch (...) {
            res.status = 400;
            res.set_content("{\"error\":\"invalid JSON body\"}",
                            "application/json");
            return;
        }

        std::string cidr = body.value("cidr", "");
        if (cidr.empty()) {
            res.status = 400;
            res.set_content("{\"error\":\"cidr field is required\"}",
                            "application/json");
            return;
        }

        std::string err = config_api::route_add(config_mgr_, cidr);
        if (!err.empty()) {
            res.status = 400;
            nlohmann::json err_resp;
            err_resp["error"] = err;
            res.set_content(err_resp.dump(), "application/json");
            return;
        }

        Config cfg = config_mgr_.load();
        nlohmann::json arr = nlohmann::json::array();
        for (const auto& r : cfg.routes) {
            arr.push_back({{"cidr", r}});
        }
        res.set_content(arr.dump(), "application/json");
    });

    // ── DELETE /api/routes ──────────────────────────────────────────
    server_->Delete("/api/routes", [this](const httplib::Request& req,
                                           httplib::Response& res) {
        nlohmann::json body;
        try {
            body = nlohmann::json::parse(req.body);
        } catch (...) {
            res.status = 400;
            res.set_content("{\"error\":\"invalid JSON body\"}",
                            "application/json");
            return;
        }

        std::string cidr = body.value("cidr", "");
        if (cidr.empty()) {
            res.status = 400;
            res.set_content("{\"error\":\"cidr field is required\"}",
                            "application/json");
            return;
        }

        std::string err = config_api::route_remove(config_mgr_, cidr);
        if (!err.empty()) {
            res.status = 400;
            nlohmann::json err_resp;
            err_resp["error"] = err;
            res.set_content(err_resp.dump(), "application/json");
            return;
        }

        Config cfg = config_mgr_.load();
        nlohmann::json arr = nlohmann::json::array();
        for (const auto& r : cfg.routes) {
            arr.push_back({{"cidr", r}});
        }
        res.set_content(arr.dump(), "application/json");
    });

    // ── POST /api/routes/reset ──────────────────────────────────────
    server_->Post("/api/routes/reset", [this](const httplib::Request&,
                                                httplib::Response& res) {
        config_api::route_reset_defaults(config_mgr_);
        Config cfg = config_mgr_.load();
        nlohmann::json arr = nlohmann::json::array();
        for (const auto& r : cfg.routes) {
            arr.push_back({{"cidr", r}});
        }
        res.set_content(arr.dump(), "application/json");
    });

    // ── GET /api/key ────────────────────────────────────────────────
    server_->Get("/api/key", [this](const httplib::Request&,
                                     httplib::Response& res) {
        std::string status_str = config_api::key_status();
        nlohmann::json j;
        j["present"] = (status_str == "valid");
        j["fingerprint"] = (status_str == "valid") ? "active" : nullptr;
        j["status"] = status_str;
        res.set_content(j.dump(), "application/json");
    });

    // ── GET /api/config/key ─────────────────────────────────────────
    server_->Get("/api/config/key", [this](const httplib::Request&,
                                             httplib::Response& res) {
        std::string status_str = config_api::key_status();
        nlohmann::json j;
        j["present"] = (status_str == "valid");
        j["fingerprint"] = (status_str == "valid") ? "active" : nullptr;
        j["status"] = status_str;
        res.set_content(j.dump(), "application/json");
    });

    // ── POST /api/key/reset ─────────────────────────────────────────
    server_->Post("/api/key/reset", [this](const httplib::Request&,
                                            httplib::Response& res) {
        config_api::key_reset_noninteractive();
        std::string status_str = config_api::key_status();
        nlohmann::json j;
        j["present"] = (status_str == "valid");
        j["fingerprint"] = (status_str == "valid") ? "active" : nullptr;
        j["status"] = status_str;
        res.set_content(j.dump(), "application/json");
    });

    // ── GET /api/service ────────────────────────────────────────────
    server_->Get("/api/service", [this](const httplib::Request&,
                                         httplib::Response& res) {
        bool available = helper::is_available();
        nlohmann::json j;
        j["installed"] = utils::file_exists("/Library/LaunchDaemons/com.ecnu.exv.helper.plist");
        j["running"] = available;
        j["path"] = "/usr/local/bin/exv";
        j["available"] = available;
        res.set_content(j.dump(), "application/json");
    });

    // ── POST /api/service/install ───────────────────────────────────
    server_->Post("/api/service/install",
                   [this](const httplib::Request&,
                          httplib::Response& res) {
                                      res.status = 400;
                       res.set_content(
                           "{\"error\":\"Service installation requires root "
                           "privileges and cannot be performed from the WebUI\"}",
                           "application/json");
                   });

    // ── POST /api/service/uninstall ─────────────────────────────────
    server_->Post("/api/service/uninstall",
                   [this](const httplib::Request&,
                          httplib::Response& res) {
                                      res.status = 400;
                       res.set_content(
                           "{\"error\":\"Service uninstallation requires root "
                           "privileges and cannot be performed from the WebUI\"}",
                           "application/json");
                   });

    // ── GET /api/logs?lines=N&filter=STR ────────────────────────────
    server_->Get("/api/logs", [this](const httplib::Request& req,
                                      httplib::Response& res) {
        Config cfg = config_mgr_.load();
        std::string log_path = utils::expand_home(cfg.log_file);

        int max_lines = 100;
        if (req.has_param("lines")) {
            try {
                max_lines = std::stoi(req.get_param_value("lines"));
                if (max_lines < 1) max_lines = 1;
                if (max_lines > 10000) max_lines = 10000;
            } catch (...) {
            }
        }

        std::string filter;
        if (req.has_param("filter")) {
            filter = req.get_param_value("filter");
        }

        nlohmann::json j;
        j["lines"] = nlohmann::json::array();

        if (utils::file_exists(log_path)) {
            std::ifstream ifs(log_path);
            if (ifs.is_open()) {
                std::vector<std::string> all_lines;
                std::string line;
                while (std::getline(ifs, line)) {
                    if (!line.empty() && line.back() == '\r')
                        line.pop_back();
                    if (filter.empty() ||
                        line.find(filter) != std::string::npos) {
                        all_lines.push_back(line);
                    }
                }

                int start = 0;
                if (static_cast<int>(all_lines.size()) > max_lines) {
                    start = static_cast<int>(all_lines.size()) - max_lines;
                }
                for (int i = start;
                     i < static_cast<int>(all_lines.size()); ++i) {
                    j["lines"].push_back(all_lines[i]);
                }
            }
        }

        j["total"] = j["lines"].size();
        j["path"] = log_path;
        res.set_content(j.dump(), "application/json");
    });

    // ── GET /api/logs/stream (SSE) ──────────────────────────────────
    server_->Get("/api/logs/stream", [this](const httplib::Request&,
                                             httplib::Response& res) {
        int client_id = log_broadcaster_.add_client();
        if (client_id < 0) {
            res.status = 503;
            res.set_content(
                "{\"error\":\"SSE log stream connection limit reached\"}",
                "application/json");
            return;
        }

        auto active = std::make_shared<std::atomic<bool>>(true);

        res.set_chunked_content_provider(
            "text/event-stream",
            [this, client_id, active](size_t /*offset*/,
                                       httplib::DataSink& sink) -> bool {
                if (!(*active)) return false;

                std::string event =
                    log_broadcaster_.next_event(client_id, 1000);
                if (!event.empty()) {
                    sink.write(event.data(), event.size());
                }
                return *active && running_;
            },
            [this, client_id, active](bool /*success*/) {
                *active = false;
                log_broadcaster_.remove_client(client_id);
            });
    });

    // ── GET /api/status/stream (SSE) ────────────────────────────────
    server_->Get("/api/status/stream", [this](const httplib::Request&,
                                               httplib::Response& res) {
        int client_id = status_broadcaster_.add_client();
        if (client_id < 0) {
            res.status = 503;
            res.set_content(
                "{\"error\":\"SSE status stream connection limit reached\"}",
                "application/json");
            return;
        }

        auto active = std::make_shared<std::atomic<bool>>(true);

        res.set_chunked_content_provider(
            "text/event-stream",
            [this, client_id, active](size_t /*offset*/,
                                       httplib::DataSink& sink) -> bool {
                if (!(*active)) return false;

                std::string event =
                    status_broadcaster_.next_event(client_id, 1000);
                if (!event.empty()) {
                    sink.write(event.data(), event.size());
                }
                return *active && running_;
            },
            [this, client_id, active](bool /*success*/) {
                *active = false;
                status_broadcaster_.remove_client(client_id);
            });
    });

    // ── GET /api/events (combined SSE) ──────────────────────────────
    server_->Get("/api/events", [this](const httplib::Request&,
                                         httplib::Response& res) {
        int log_client = log_broadcaster_.add_client();
        int status_client = status_broadcaster_.add_client();

        if (log_client < 0 && status_client < 0) {
            res.status = 503;
            res.set_content(
                "{\"error\":\"SSE connection limit reached\"}",
                "application/json");
            return;
        }

        auto active = std::make_shared<std::atomic<bool>>(true);

        res.set_chunked_content_provider(
            "text/event-stream",
            [this, log_client, status_client, active](
                size_t /*offset*/, httplib::DataSink& sink) -> bool {
                if (!(*active)) return false;

                // Check log events
                if (log_client >= 0) {
                    std::string log_event =
                        log_broadcaster_.next_event(log_client, 100);
                    if (!log_event.empty()) {
                        sink.write(log_event.data(), log_event.size());
                    }
                }

                // Check status events
                if (status_client >= 0) {
                    std::string status_event =
                        status_broadcaster_.next_event(status_client, 100);
                    if (!status_event.empty()) {
                        sink.write(status_event.data(), status_event.size());
                    }
                }

                return *active && running_;
            },
            [this, log_client, status_client, active](bool /*success*/) {
                *active = false;
                if (log_client >= 0)
                    log_broadcaster_.remove_client(log_client);
                if (status_client >= 0)
                    status_broadcaster_.remove_client(status_client);
            });
    });

    // ── Static file serving (catch-all GET) ─────────────────────────
    server_->Get(".*", [this](const httplib::Request& req,
                               httplib::Response& res) {
        // Let API routes be handled by their specific handlers above
        // (This catch-all only fires for paths not matched by /api/*)
        if (req.path.find("/api/") == 0) {
            res.status = 404;
            res.set_content("{\"error\":\"not found\"}", "application/json");
            return;
        }

        serve_static(req, res);
    });
}

// ── Lifecycle ───────────────────────────────────────────────────────

void WebUIServer::start() {
    if (running_) return;
    running_ = true;

    std::ostringstream ss;
    ss << "WebUI available at http://" << bind_address_ << ":" << port_
       << "/";
    std::cout << ss.str() << std::endl;

    server_thread_ = std::thread([this]() {
        server_->listen(bind_address_.c_str(), port_);
    });
}

void WebUIServer::stop() {
    if (!running_) return;
    running_ = false;

    server_->stop();

    if (server_thread_.joinable()) {
        server_thread_.join();
    }

    // Wait for in-flight async VPN tasks (max 5 seconds)
    {
        std::lock_guard<std::mutex> lock(tasks_mutex_);
        for (auto& future : async_tasks_) {
            if (future.valid()) {
                future.wait_for(std::chrono::seconds(5));
            }
        }
    }

    vpn_connecting_ = false;
    vpn_disconnecting_ = false;
}

bool WebUIServer::is_running() const { return running_; }

} // namespace webui
} // namespace ecnuvpn