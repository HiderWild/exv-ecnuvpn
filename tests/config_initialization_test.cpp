#include "core/config/config_initialization.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

using json = nlohmann::json;

namespace {

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

std::filesystem::path unique_temp_dir(const char *name) {
  const auto now = std::chrono::steady_clock::now().time_since_epoch().count();
  auto dir = std::filesystem::temp_directory_path() /
             (std::string(name) + "-" + std::to_string(now));
  std::filesystem::create_directories(dir);
  return dir;
}

std::filesystem::path config_path(const std::filesystem::path &dir) {
  return dir / "config.json";
}

void write_json(const std::filesystem::path &dir, const json &value) {
  std::ofstream out(config_path(dir), std::ios::out | std::ios::trunc);
  out << value.dump(2);
}

json read_json(const std::filesystem::path &dir) {
  std::ifstream in(config_path(dir));
  json parsed;
  in >> parsed;
  return parsed;
}

json complete_config() {
  return {
      {"server", "https://vpn-ct.ecnu.edu.cn"},
      {"username", ""},
      {"password", ""},
      {"mtu", 1290},
      {"useragent", "test-agent"},
      {"disable_dtls", false},
      {"remember_password", false},
      {"routes", json::array({"49.52.4.0/25"})},
      {"extra_args", json::array()},
      {"log_file", ""},
      {"vpn_engine", "native"},
      {"windows_tunnel_driver", "auto"},
      {"windows_tap_interface", ""},
      {"auto_reconnect", true},
      {"minimal_mode", false},
      {"service_install_prompt_seen", false},
      {"minimal_install_service_before_connect", true},
      {"include_class_a_private_routes", false},
      {"include_class_b_private_routes", false},
      {"launch_at_login", false},
      {"auto_connect_on_launch", false},
  };
}

bool has_minimal_required_fields(const json &cfg) {
  const char *fields[] = {
      "server",
      "username",
      "password",
      "mtu",
      "useragent",
      "disable_dtls",
      "remember_password",
      "routes",
      "extra_args",
      "log_file",
      "vpn_engine",
      "windows_tunnel_driver",
      "windows_tap_interface",
      "auto_reconnect",
      "minimal_mode",
      "service_install_prompt_seen",
      "minimal_install_service_before_connect",
      "include_class_a_private_routes",
      "include_class_b_private_routes",
      "launch_at_login",
      "auto_connect_on_launch",
  };
  for (const char *field : fields) {
    if (!cfg.contains(field)) {
      std::cerr << "missing field: " << field << '\n';
      return false;
    }
  }
  return true;
}

} // namespace

int main() {
  bool ok = true;

  {
    auto dir = unique_temp_dir("exv-config-init-missing");
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Missing,
                "missing config reports Missing") &&
         ok;
    ok = expect(result.should_request_quick_start(),
                "missing config requests quick start") &&
         ok;
    ok = expect(std::filesystem::exists(config_path(dir)),
                "missing config writes config.json") &&
         ok;
    ok = expect(has_minimal_required_fields(read_json(dir)),
                "missing config writes every minimal field") &&
         ok;
    ok = expect(read_json(dir).value("remember_password", true) == false,
                "missing config writes remember_password false by default") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  {
    auto dir = unique_temp_dir("exv-config-init-invalid-json");
    {
      std::ofstream out(config_path(dir), std::ios::out | std::ios::trunc);
      out << "{not-json";
    }
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Invalid,
                "parse failure reports Invalid") &&
         ok;
    ok = expect(result.should_request_quick_start(),
                "parse failure requests quick start") &&
         ok;
    ok = expect(has_minimal_required_fields(read_json(dir)),
                "parse failure replaces file with complete config") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  {
    auto dir = unique_temp_dir("exv-config-init-incomplete");
    write_json(dir, json{{"server", "https://vpn.example.invalid"},
                         {"username", ""},
                         {"password", ""}});
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Invalid,
                "missing initialized fields reports Invalid") &&
         ok;
    ok = expect(result.reason.find("missing:mtu") != std::string::npos,
                "missing field reason names mtu") &&
         ok;
    ok = expect(has_minimal_required_fields(read_json(dir)),
                "incomplete config is replaced with complete defaults") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  {
    auto dir = unique_temp_dir("exv-config-init-wrong-type");
    auto cfg = complete_config();
    cfg["mtu"] = "1290";
    write_json(dir, cfg);
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Invalid,
                "wrong JSON type reports Invalid") &&
         ok;
    ok = expect(result.reason.find("type:mtu") != std::string::npos,
                "wrong type reason names mtu") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  {
    auto dir = unique_temp_dir("exv-config-init-complete");
    write_json(dir, complete_config());
    auto result = exv::config::ensure_initialized_config(dir.string());
    ok = expect(result.status == exv::config::ConfigInitializationStatus::Normal,
                "complete config reports Normal") &&
         ok;
    ok = expect(!result.should_request_quick_start(),
                "complete config does not request quick start") &&
         ok;
    ok = expect(read_json(dir).at("server") == "https://vpn-ct.ecnu.edu.cn",
                "complete config is not replaced") &&
         ok;
    std::filesystem::remove_all(dir);
  }

  if (ok) {
    std::cout << "config_initialization_test: all assertions passed\n";
  } else {
    std::cerr << "config_initialization_test: some assertions FAILED\n";
  }
  return ok ? 0 : 1;
}
