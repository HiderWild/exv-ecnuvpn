#include "cli/cli_commands.hpp"

#include "cli/core_request_formatter.hpp"

#include <nlohmann/json.hpp>

#include <algorithm>
#include <iostream>
#include <optional>
#include <sstream>

namespace exv::cli {
namespace {

struct ParsedArgs {
  std::vector<std::string> positional;
  int retry_limit = 0;
  bool retry_specified = false;
  std::string error;
};

std::optional<int> try_parse_int(const std::string &value) {
  try {
    size_t parsed = 0;
    int result = std::stoi(value, &parsed);
    if (parsed != value.size()) {
      return std::nullopt;
    }
    return result;
  } catch (...) {
    return std::nullopt;
  }
}

ParsedArgs parse_args(const std::vector<std::string> &args) {
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
        auto value = try_parse_int(args[i + 1]);
        if (value.has_value()) {
          if (*value < -1) {
            parsed.error = "-rt only accepts -1, 0, or positive.";
            return parsed;
          }
          parsed.retry_limit = *value;
          ++i;
        }
      }
      continue;
    }
    parsed.positional.push_back(arg);
  }
  return parsed;
}

std::ostream &out_stream(const CliCommandDeps &deps) {
  return deps.out ? *deps.out : std::cout;
}

std::ostream &err_stream(const CliCommandDeps &deps) {
  return deps.err ? *deps.err : std::cerr;
}

bool response_ok(const std::string &line, std::ostream &err) {
  if (line.empty()) {
    err << "Core did not return a response.\n";
    return false;
  }
  try {
    auto response = nlohmann::json::parse(line);
    if (response.value("success", false)) {
      return true;
    }
    err << response.value("error_message", response.value("error", "Core request failed.")) << '\n';
  } catch (const std::exception &e) {
    err << "Invalid core response: " << e.what() << '\n';
  }
  return false;
}

int send_action(const CliCommandDeps &deps,
                const CliCoreResolution &core,
                const std::string &action,
                const nlohmann::json &payload = nlohmann::json::object()) {
  if (!deps.send_core_request) {
    err_stream(deps) << "Core IPC client is unavailable.\n";
    return 1;
  }
  const std::string response =
      deps.send_core_request(core.ipc_path, format_core_request(action, payload));
  return response_ok(response, err_stream(deps)) ? 0 : 1;
}

std::optional<CliCoreResolution> resolve_or_fail(const CliCommandDeps &deps) {
  if (!deps.resolve_core) {
    err_stream(deps) << "Core resolver is unavailable.\n";
    return std::nullopt;
  }
  CliCoreResolution core = deps.resolve_core();
  if (!core.ok) {
    err_stream(deps) << (core.message.empty() ? "Unable to resolve core." : core.message) << '\n';
    return std::nullopt;
  }
  return core;
}

int handle_start(const CliCommandDeps &deps, const ParsedArgs &parsed) {
  auto core = resolve_or_fail(deps);
  if (!core.has_value()) {
    return 1;
  }
  if (parsed.retry_specified) {
    nlohmann::json payload;
    payload["settings"] = nlohmann::json{{"retry_limit", parsed.retry_limit}};
    int rc = send_action(deps, *core, "config.saveSettings", payload);
    if (rc != 0) {
      return rc;
    }
  }
  return send_action(deps, *core, "vpn.connect");
}

int handle_config(const std::vector<std::string> &args, const CliCommandDeps &deps) {
  const std::string subcmd = args.size() > 2 ? args[2] : "show";

  if (subcmd == "show") {
    auto core = resolve_or_fail(deps);
    return core.has_value() ? send_action(deps, *core, "config.get") : 1;
  }

  if (subcmd == "set") {
    if (args.size() < 5) {
      err_stream(deps) << "Usage: exv-cli config set <key> <value>\n";
      return 1;
    }
    const std::string &key = args[3];
    const std::string &value = args[4];
    std::string action;
    nlohmann::json payload;
    if (key == "server" || key == "username") {
      action = "config.saveAuth";
      payload[key] = value;
    } else if (key == "mtu" || key == "retry_limit") {
      auto parsed_value = try_parse_int(value);
      if (!parsed_value.has_value()) {
        err_stream(deps) << "Config key '" << key << "' requires an integer value.\n";
        return 1;
      }
      action = "config.saveSettings";
      payload["settings"] = nlohmann::json{{key, *parsed_value}};
    } else if (key == "dtls" || key == "auto_reconnect") {
      if (value != "true" && value != "false") {
        err_stream(deps) << "Config key '" << key << "' requires true or false.\n";
        return 1;
      }
      action = "config.saveSettings";
      payload["settings"] = nlohmann::json{{key, value == "true"}};
    } else {
      err_stream(deps) << "Unknown config key: " << key << '\n';
      return 1;
    }

    auto core = resolve_or_fail(deps);
    return core.has_value() ? send_action(deps, *core, action, payload) : 1;
  }

  if (subcmd == "reset") {
    const bool explicit_confirm =
        std::find(args.begin() + 3, args.end(), "--confirm") != args.end();
    if (!explicit_confirm) {
      if (!deps.interactive) {
        err_stream(deps) << "config reset requires --confirm in non-interactive mode.\n";
        return 1;
      }
      if (!deps.confirm || !deps.confirm("Reset configuration?")) {
        err_stream(deps) << "Configuration reset cancelled.\n";
        return 1;
      }
    }
    auto core = resolve_or_fail(deps);
    return core.has_value() ? send_action(deps, *core, "config.reset") : 1;
  }

  err_stream(deps) << "Unknown config subcommand: " << subcmd << '\n';
  return 1;
}

void print_help(std::ostream &out) {
  out << "Usage: exv-cli [command] [options]\n"
      << "Commands: start, stop, status, config, service, version, help\n";
}

} // namespace

int run_cli_command(const std::vector<std::string> &raw_args, CliCommandDeps deps) {
  ParsedArgs parsed = parse_args(raw_args);
  if (!parsed.error.empty()) {
    err_stream(deps) << parsed.error << '\n';
    return 1;
  }

  std::vector<std::string> args;
  args.emplace_back(raw_args.empty() ? "exv-cli" : raw_args[0]);
  args.insert(args.end(), parsed.positional.begin(), parsed.positional.end());
  if (args.size() <= 1) {
    args.push_back("start");
  }

  const std::string cmd = args[1];
  if (parsed.retry_specified && cmd != "start") {
    err_stream(deps) << "-rt can only be used with start.\n";
    return 1;
  }

  if (cmd == "help" || cmd == "-h" || cmd == "--help") {
    print_help(out_stream(deps));
    return 0;
  }
  if (cmd == "version" || cmd == "-v" || cmd == "--version") {
    if (!deps.version_probe) {
      err_stream(deps) << "Core version probe is unavailable.\n";
      return 1;
    }
    out_stream(deps) << deps.version_probe();
    return 0;
  }
  if (cmd == "start") {
    return handle_start(deps, parsed);
  }
  if (cmd == "stop" || cmd == "-s") {
    auto core = resolve_or_fail(deps);
    return core.has_value() ? send_action(deps, *core, "vpn.disconnect") : 1;
  }
  if (cmd == "status" || cmd == "-t") {
    auto core = resolve_or_fail(deps);
    return core.has_value() ? send_action(deps, *core, "status.get") : 1;
  }
  if (cmd == "config" || cmd == "-c") {
    return handle_config(args, deps);
  }
  if (cmd == "service") {
    const std::string subcmd = args.size() > 2 ? args[2] : "status";
    if (subcmd == "install") {
      auto core = resolve_or_fail(deps);
      return core.has_value() ? send_action(deps, *core, "service.install") : 1;
    }
    if (subcmd == "uninstall") {
      auto core = resolve_or_fail(deps);
      return core.has_value() ? send_action(deps, *core, "service.uninstall") : 1;
    }
    if (subcmd == "status") {
      auto core = resolve_or_fail(deps);
      return core.has_value() ? send_action(deps, *core, "service.status") : 1;
    }
    err_stream(deps) << "Unknown service subcommand: " << subcmd << '\n';
    return 1;
  }

  err_stream(deps) << "Unknown command: " << cmd << '\n';
  return 1;
}

} // namespace exv::cli
