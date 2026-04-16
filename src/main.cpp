#include "config.hpp"
#include "logger.hpp"
#include "utils.hpp"
#include "vpn.hpp"

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
    if (parsed != value.size())
      return std::nullopt;
    return result;
  } catch (...) {
    return std::nullopt;
  }
}

static ParsedArgs parse_args(const std::vector<std::string> &args) {
  ParsedArgs parsed;

  for (size_t i = 1; i < args.size(); ++i) {
    const std::string &arg = args[i];
    if (arg == "-rt") {
      if (parsed.retry_specified) {
        parsed.error = "-rt can only be specified once.";
        return parsed;
      }

      parsed.retry_specified = true;
      parsed.retry_limit = -1;

      if (i + 1 < args.size()) {
        auto maybe_value = try_parse_int(args[i + 1]);
        if (maybe_value.has_value()) {
          if (*maybe_value < -1) {
            parsed.error = "-rt only accepts -1, 0, or a positive integer.";
            return parsed;
          }
          parsed.retry_limit = *maybe_value;
          ++i;
        }
      }
      continue;
    }

    parsed.positional.push_back(arg);
  }

  return parsed;
}

static void print_help() {
  std::cout << std::endl;
  std::cout << utils::BOLD << utils::CYAN
            << "  ╔═══════════════════════════════════════╗" << std::endl
            << "  ║             EXV Client  v" << ECNUVPN_VERSION
            << "        ║" << std::endl
            << "  ╚═══════════════════════════════════════╝" << utils::RESET
            << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "USAGE:" << utils::RESET << std::endl;
  std::cout << "  exv [command] [options]" << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "COMMANDS:" << utils::RESET << std::endl;
  std::cout << "  " << utils::GREEN << "start" << utils::RESET << " / "
            << utils::GREEN << "(default)" << utils::RESET
            << "         Start VPN connection" << std::endl;
  std::cout << "  " << utils::GREEN << "stop, -s" << utils::RESET
            << "                   Stop VPN connection" << std::endl;
  std::cout << "  " << utils::GREEN << "status, -t" << utils::RESET
            << "                 Show VPN status" << std::endl;
  std::cout << "  " << utils::GREEN << "config, -c" << utils::RESET
            << "                 Manage configuration" << std::endl;
  std::cout << "  " << utils::GREEN << "logs, -l" << utils::RESET
            << "                   View recent logs" << std::endl;
  std::cout << "  " << utils::GREEN << "help, -h" << utils::RESET
            << "                   Show this help" << std::endl;
  std::cout << "  " << utils::GREEN << "version, -v" << utils::RESET
            << "                Show version" << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "START OPTIONS:" << utils::RESET << std::endl;
  std::cout << "  " << utils::YELLOW << "-rt [count]" << utils::RESET
            << "               Reconnect count after disconnect" << std::endl;
  std::cout << "                           Omit count or use -1 for infinite"
            << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "CONFIG SUBCOMMANDS:" << utils::RESET
            << std::endl;
  std::cout << "  " << utils::YELLOW << "config show" << utils::RESET
            << "                 Show current config" << std::endl;
  std::cout << "  " << utils::YELLOW << "config import <file>" << utils::RESET
            << "        Import from JSON file" << std::endl;
  std::cout << "  " << utils::YELLOW << "config set <key>" << utils::RESET
            << "            Set a config value (interactive)" << std::endl;
  std::cout << "  " << utils::YELLOW << "config reset" << utils::RESET
            << "                Reset to defaults (key preserved)" << std::endl;
  std::cout << "  " << utils::YELLOW << "config routes list" << utils::RESET
            << "          List all routes" << std::endl;
  std::cout << "  " << utils::YELLOW << "config routes add <cidr>"
            << utils::RESET << "    Add a route" << std::endl;
  std::cout << "  " << utils::YELLOW << "config routes remove <cidr>"
            << utils::RESET << " Remove a route" << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "KEY SUBCOMMANDS:" << utils::RESET << std::endl;
  std::cout << "  " << utils::YELLOW << "config key show" << utils::RESET
            << "             Show key file status" << std::endl;
  std::cout << "  " << utils::YELLOW << "config key reset" << utils::RESET
            << "            Regenerate key (clears password)" << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "EXAMPLES:" << utils::RESET << std::endl;
  std::cout
      << utils::DIM << "  sudo exv                               # Start VPN"
      << std::endl
      << "  sudo exv -rt 3                         # Retry reconnect 3 times"
      << std::endl
      << "  sudo exv -rt                           # Retry reconnect forever"
      << std::endl
      << "  sudo exv stop                          # Stop VPN" << std::endl
      << "  exv status                             # Check status" << std::endl
      << "  exv config set username                # Set username" << std::endl
      << "  exv config set password                # Set password (hidden)"
      << std::endl
      << "  exv config import ~/vpn.json           # Import config"
      << std::endl
      << "  exv config routes add 10.0.0.0/8       # Add route" << std::endl
      << "  exv config key reset                   # Regenerate key"
      << std::endl
      << utils::RESET << std::endl;
}

static void print_version() {
  std::cout << utils::BOLD << APP_NAME << utils::RESET << " version "
            << utils::GREEN << ECNUVPN_VERSION << utils::RESET << std::endl;
}

static int handle_config(const std::vector<std::string> &args) {
  // exv config (no subcommand) -> show
  if (args.size() <= 2) {
    Config cfg = config::load();
    config::show(cfg);
    return 0;
  }

  std::string subcmd = args[2];

  if (subcmd == "show") {
    Config cfg = config::load();
    config::show(cfg);
    return 0;
  }

  if (subcmd == "import") {
    if (args.size() < 4) {
      utils::print_error("Usage: exv config import <file>");
      return 1;
    }
    config::import_from(args[3]);
    return 0;
  }

  if (subcmd == "set") {
    if (args.size() < 4) {
      utils::print_error("Usage: exv config set <key>");
      utils::print_info(
          "Keys: server, username, password, mtu, useragent, log_file");
      return 1;
    }
    Config cfg = config::load();
    // set_value for password prompts hidden input internally
    return config::set_value(cfg, args[3]) ? 0 : 1;
  }

  if (subcmd == "reset") {
    config::reset();
    return 0;
  }

  if (subcmd == "key") {
    if (args.size() < 4) {
      config::key_show();
      return 0;
    }
    std::string key_cmd = args[3];
    if (key_cmd == "show") {
      config::key_show();
      return 0;
    }
    if (key_cmd == "reset") {
      return config::key_reset() ? 0 : 1;
    }
    utils::print_error("Unknown key subcommand: " + key_cmd);
    utils::print_info("Available: show, reset");
    return 1;
  }

  if (subcmd == "routes") {
    if (args.size() < 4) {
      Config cfg = config::load();
      config::list_routes(cfg);
      return 0;
    }
    std::string route_cmd = args[3];

    if (route_cmd == "list") {
      Config cfg = config::load();
      config::list_routes(cfg);
      return 0;
    }
    if (route_cmd == "add") {
      if (args.size() < 5) {
        utils::print_error("Usage: exv config routes add <cidr>");
        return 1;
      }
      Config cfg = config::load();
      return config::add_route(cfg, args[4]) ? 0 : 1;
    }
    if (route_cmd == "remove") {
      if (args.size() < 5) {
        utils::print_error("Usage: exv config routes remove <cidr>");
        return 1;
      }
      Config cfg = config::load();
      return config::remove_route(cfg, args[4]) ? 0 : 1;
    }
    utils::print_error("Unknown routes subcommand: " + route_cmd);
    utils::print_info("Available: list, add, remove");
    return 1;
  }

  utils::print_error("Unknown config subcommand: " + subcmd);
  utils::print_info("Available: show, import, set, reset, routes, key");
  return 1;
}

int main(int argc, char *argv[]) {
  logger::init();

  std::vector<std::string> raw_args;
  for (int i = 0; i < argc; ++i) {
    raw_args.emplace_back(argv[i]);
  }

  ParsedArgs parsed = parse_args(raw_args);
  if (!parsed.error.empty()) {
    utils::print_error(parsed.error);
    return 1;
  }

  std::vector<std::string> args;
  args.emplace_back(raw_args[0]);
  args.insert(args.end(), parsed.positional.begin(), parsed.positional.end());

  // No arguments -> start VPN
  if (args.size() <= 1) {
    Config cfg = config::load();
    return vpn::start(cfg, parsed.retry_limit);
  }

  std::string cmd = args[1];

  if (parsed.retry_specified && cmd != "start") {
    utils::print_error("-rt can only be used with the default start action or 'start'.");
    return 1;
  }

  if (cmd == "help" || cmd == "-h" || cmd == "--help") {
    print_help();
    return 0;
  }
  if (cmd == "version" || cmd == "-v" || cmd == "--version") {
    print_version();
    return 0;
  }
  if (cmd == "start") {
    Config cfg = config::load();
    return vpn::start(cfg, parsed.retry_limit);
  }
  if (cmd == "stop" || cmd == "-s") {
    return vpn::stop();
  }
  if (cmd == "status" || cmd == "-t") {
    return vpn::status();
  }
  if (cmd == "config" || cmd == "-c") {
    return handle_config(args);
  }
  if (cmd == "logs" || cmd == "-l") {
    logger::show_logs();
    return 0;
  }

  utils::print_error("Unknown command: " + cmd);
  utils::print_info("Run 'exv help' for usage information.");
  return 1;
}
