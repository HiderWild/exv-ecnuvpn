#include "app_api.hpp"
#include "core/config/config.hpp"
#include "core/config/config_manager.hpp"
#include "core/core_process.hpp"
#include "helper/helper.hpp"
#include "logger.hpp"
#include "runtime/runtime_context.hpp"
#include "tunnel.hpp"
#include "cli/console.hpp"
#include "utils.hpp"
#include "vpn.hpp"
#include "virtual_network.hpp"

#include <csignal>
#include <iostream>
#include <optional>
#include <string>
#include <vector>

using namespace ecnuvpn;

static constexpr const char *APP_NAME = "exv";

struct ParsedArgs {
  std::vector<std::string> positional;
  int retry_limit = 0;
  bool retry_specified = false;
  std::string error;
};

static std::optional<int> try_parse_int(const std::string &value) {
  try {
    size_t parsed = 0;
    int result = std::stoi(value, &parsed);
    if (parsed != value.size()) return std::nullopt;
    return result;
  } catch (...) { return std::nullopt; }
}

static ParsedArgs parse_args(const std::vector<std::string> &args) {
  ParsedArgs parsed;
  for (size_t i = 1; i < args.size(); ++i) {
    const std::string &arg = args[i];
    if (arg == "-rt") {
      if (parsed.retry_specified) { parsed.error = "-rt can only be specified once."; return parsed; }
      parsed.retry_specified = true; parsed.retry_limit = -1;
      if (i + 1 < args.size()) {
        auto v = try_parse_int(args[i + 1]);
        if (v.has_value()) { if (*v < -1) { parsed.error = "-rt only accepts -1, 0, or positive."; return parsed; } parsed.retry_limit = *v; ++i; }
      }
      continue;
    }
    parsed.positional.push_back(arg);
  }
  return parsed;
}

static void print_help() {
  std::cout << std::endl << cli::BOLD << cli::CYAN << "             EXV Client  v" << ECNUVPN_VERSION << cli::RESET << std::endl << std::endl;
  std::cout << cli::BOLD << "USAGE:" << cli::RESET << std::endl << "  exv [command] [options]" << std::endl << std::endl;
  std::cout << cli::BOLD << "COMMANDS:" << cli::RESET << std::endl;
  std::cout << "  " << cli::GREEN << "start" << cli::RESET << " / " << cli::GREEN << "(default)" << cli::RESET << "         Start VPN connection" << std::endl;
  std::cout << "  " << cli::GREEN << "stop, -s" << cli::RESET << "                   Stop VPN connection" << std::endl;
  std::cout << "  " << cli::GREEN << "status, -t" << cli::RESET << "                 Show VPN status" << std::endl;
  std::cout << "  " << cli::GREEN << "config, -c" << cli::RESET << "                 Manage configuration" << std::endl;
  std::cout << "  " << cli::GREEN << "service" << cli::RESET << "                    Manage the root helper service" << std::endl;
  std::cout << "  " << cli::GREEN << "logs, -l" << cli::RESET << "                   View recent logs" << std::endl;
  std::cout << "  " << cli::GREEN << "help, -h" << cli::RESET << "                   Show this help" << std::endl;
  std::cout << "  " << cli::GREEN << "version, -v" << cli::RESET << "                Show version" << std::endl;
  std::cout << std::endl;
  std::cout << cli::BOLD << "START OPTIONS:" << cli::RESET << std::endl;
  std::cout << "  " << cli::YELLOW << "-rt [count]" << cli::RESET << "               Reconnect count after disconnect" << std::endl;
  std::cout << std::endl << "  Note: Desktop app (Electron) is the recommended interface." << std::endl << std::endl;
}

static void print_version() {
  std::cout << cli::BOLD << APP_NAME << cli::RESET << " version " << cli::GREEN << ECNUVPN_VERSION << cli::RESET << std::endl;
}

static int handle_service(const std::vector<std::string> &args) {
  if (args.size() <= 2 || args[2] == "status") return helper::show_service_status();
  std::string subcmd = args[2];
  if (subcmd == "install") return helper::install_service(utils::get_executable_path());
  if (subcmd == "uninstall") return helper::uninstall_service();
  cli::print_error("Unknown service subcommand: " + subcmd); return 1;
}

static int handle_config(const std::vector<std::string> &args) {
  if (args.size() <= 2) { Config cfg = config::load(); config::show(cfg); return 0; }
  std::string subcmd = args[2];
  if (subcmd == "show") { Config cfg = config::load(); config::show(cfg); return 0; }
  if (subcmd == "import") { if (args.size() < 4) { cli::print_error("Usage: exv config import <file>"); return 1; } config::import_from(args[3]); return 0; }
  if (subcmd == "set") { if (args.size() < 4) { cli::print_error("Usage: exv config set <key> [value]"); return 1; } Config cfg = config::load(); std::string v = (args.size() >= 5) ? args[4] : ""; return config::set_value(cfg, args[3], v) ? 0 : 1; }
  if (subcmd == "reset") { config::reset(); return 0; }
  if (subcmd == "key") { if (args.size() < 4) { config::key_show(); return 0; } std::string kc = args[3]; if (kc == "show") { config::key_show(); return 0; } if (kc == "reset") { return config::key_reset() ? 0 : 1; } cli::print_error("Unknown key subcommand: " + kc); return 1; }
  if (subcmd == "routes") {
    if (args.size() < 4) { Config cfg = config::load(); config::list_routes(cfg); return 0; }
    std::string rc = args[3];
    if (rc == "list") { Config cfg = config::load(); config::list_routes(cfg); return 0; }
    if (rc == "add") { if (args.size() < 5) { cli::print_error("Usage: exv config routes add <cidr>"); return 1; } Config cfg = config::load(); return config::add_route(cfg, args[4]) ? 0 : 1; }
    if (rc == "remove") { if (args.size() < 5) { cli::print_error("Usage: exv config routes remove <cidr>"); return 1; } Config cfg = config::load(); return config::remove_route(cfg, args[4]) ? 0 : 1; }
    cli::print_error("Unknown routes subcommand: " + rc); return 1;
  }
  cli::print_error("Unknown config subcommand: " + subcmd); return 1;
}

int main(int argc, char *argv[]) {
  cli::enable_windows_ansi();
  runtime::bootstrap();

  std::vector<std::string> raw_args;
  for (int i = 0; i < argc; ++i) raw_args.emplace_back(argv[i]);

  if (raw_args.size() > 1 && raw_args[1] == "__tunnel-script") return tunnel::run_script_hook();

  if (raw_args.size() > 1 && raw_args[1] == "--mode=core") {
    std::string config_dir, home_dir;
    bool use_stdin = true;  // Default: stdin/stdout mode for Electron
    for (size_t i = 2; i < raw_args.size(); ++i) {
      if (raw_args[i] == "--config-dir" && i + 1 < raw_args.size()) config_dir = raw_args[++i];
      else if (raw_args[i] == "--home" && i + 1 < raw_args.size()) home_dir = raw_args[++i];
      else if (raw_args[i] == "--daemon") use_stdin = false;  // Daemon mode: pipe-only
    }
    return exv::core::core_process_main(config_dir, home_dir, use_stdin);
  }

  if (raw_args.size() > 2 && (raw_args[1] == "desktop-rpc" || raw_args[1] == "desktop-rpc-file" || raw_args[1] == "desktop-rpc-file-output")) {
    const bool payload_is_file = raw_args[1] == "desktop-rpc-file" || raw_args[1] == "desktop-rpc-file-output";
    const bool output_is_file = raw_args[1] == "desktop-rpc-file-output";
    const std::string output_path = output_is_file && raw_args.size() > 4 ? raw_args[4] : "";
    auto write_rpc_result = [&](const nlohmann::json &result) {
      std::string body = result.dump();
      if (output_is_file && !output_path.empty()) return utils::write_file(output_path, body + "\n") ? 0 : 1;
      std::cout << body << std::endl; return 0;
    };
    nlohmann::json payload = nlohmann::json::object();
    if (raw_args.size() > 3) {
      try {
        if (payload_is_file) payload = nlohmann::json::parse(utils::read_file(raw_args[3]));
        else payload = nlohmann::json::parse(raw_args[3]);
      } catch (const std::exception &ex) {
        write_rpc_result(nlohmann::json{{"ok", false}, {"error", std::string("invalid JSON payload: ") + ex.what()}});
        return 1;
      }
    }
    nlohmann::json result = app_api::handle_action(raw_args[2], payload);
    int wr = write_rpc_result(result); if (wr != 0) return wr;
    if (result.is_object() && result.value("ok", true) == false) return 1;
    return 0;
  }

  ParsedArgs parsed = parse_args(raw_args);
  if (!parsed.error.empty()) { cli::print_error(parsed.error); return 1; }

  std::vector<std::string> args;
  args.emplace_back(raw_args[0]);
  args.insert(args.end(), parsed.positional.begin(), parsed.positional.end());
  if (args.size() <= 1) args.push_back("start");
  std::string cmd = args[1];

  if (parsed.retry_specified && cmd != "start") { cli::print_error("-rt can only be used with the default start action or 'start'."); return 1; }
  if (cmd == "help" || cmd == "-h" || cmd == "--help") { print_help(); return 0; }
  if (cmd == "version" || cmd == "-v" || cmd == "--version") { print_version(); return 0; }
  if (cmd == "start") { Config cfg = config::load(); return vpn::start(cfg, parsed.retry_limit); }
  if (cmd == "stop" || cmd == "-s") return vpn::stop();
  if (cmd == "status" || cmd == "-t") return vpn::status();
  if (cmd == "config" || cmd == "-c") { logger::init(); return handle_config(args); }
  if (cmd == "service") return handle_service(args);
  if (cmd == "logs" || cmd == "-l") { logger::init(); logger::show_logs(50); return 0; }

  cli::print_error("Unknown command: " + cmd);
  cli::print_info("Run 'exv help' for usage information.");
  return 1;
}
