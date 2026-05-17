#include "app_api.hpp"
#include "config.hpp"
#include "config_manager.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "sse_broadcaster.hpp"
#include "utils.hpp"
#include "vpn.hpp"
#include "virtual_network.hpp"
#include "webui.hpp"

#include <csignal>
#include <iostream>
#include <optional>
#include <string>
#ifndef _WIN32
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#else
#include <windows.h>
#endif
#include <vector>

using namespace ecnuvpn;

static constexpr const char *APP_NAME = "exv";

static volatile sig_atomic_t webui_stop_requested = 0;
static webui::WebUIServer *g_webui_server = nullptr;

static void webui_signal_handler(int) { webui_stop_requested = 1; }

struct ParsedArgs {
  std::vector<std::string> positional;
  int retry_limit = 0;
  bool retry_specified = false;
  bool foreground = false;
  bool webui = false;
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

    if (arg == "--foreground" || arg == "-f") {
      parsed.foreground = true;
      continue;
    }

    if (arg == "--webui" || arg == "-w") {
      parsed.webui = true;
      continue;
    }

    parsed.positional.push_back(arg);
  }

  return parsed;
}

static void print_help() {
  std::cout << std::endl;
  std::cout << utils::BOLD << utils::CYAN
            << "  \xe2\x95\x94\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90" << std::endl
            << "  \xe2\x95\x91             EXV Client  v" << ECNUVPN_VERSION
            << "        \xe2\x95\x91" << std::endl
            << "  \xe2\x95\x9a\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90\xe2\x95\x90" << utils::RESET
            << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "USAGE:" << utils::RESET << std::endl;
  std::cout << "  exv [command] [options]" << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "COMMANDS:" << utils::RESET << std::endl;
  std::cout << "  " << utils::GREEN << "start" << utils::RESET << " / "
            << utils::GREEN << "(default)" << utils::RESET
            << "         Start VPN connection and return" << std::endl;
  std::cout << "  " << utils::GREEN << "stop, -s" << utils::RESET
            << "                   Stop VPN connection" << std::endl;
  std::cout << "  " << utils::GREEN << "status, -t" << utils::RESET
            << "                 Show VPN status" << std::endl;
  std::cout << "  " << utils::GREEN << "config, -c" << utils::RESET
            << "                 Manage configuration" << std::endl;
  std::cout << "  " << utils::GREEN << "service" << utils::RESET
            << "                    Manage the root helper service" << std::endl;
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
  std::cout << "  " << utils::YELLOW << "-f, --foreground" << utils::RESET
            << "          Keep process alive for WebUI server" << std::endl;
  std::cout << "  " << utils::YELLOW << "-w, --webui" << utils::RESET
            << "              Start WebUI server (compatibility mode)" << std::endl;
  std::cout << std::endl;
  std::cout << "  Note: Default behavior is start VPN and return to shell."
            << std::endl;
  std::cout << "        Use --webui to launch the browser-based WebUI."
            << std::endl;
  std::cout << "        Use --foreground with --webui to keep the process attached."
            << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "VPN MODES:" << utils::RESET << std::endl;
  std::cout << "  Helper mode    VPN managed via the installed helper service."
            << std::endl;
  std::cout << "                 Recommended for daily use. No sudo/admin needed."
            << std::endl;
#ifdef _WIN32
  std::cout << "  Elevated mode  Desktop app uses one-time UAC elevation when"
            << std::endl;
  std::cout << "                 the helper is not installed, for a temporary VPN"
            << std::endl;
  std::cout << "                 session. Install the helper for persistent convenience."
            << std::endl;
#else
  std::cout << "  Elevated mode  Desktop app can request one-time sudo when"
            << std::endl;
  std::cout << "                 the helper is not installed, for a temporary VPN"
            << std::endl;
  std::cout << "                 session. Install the helper for persistent convenience."
            << std::endl;
#endif
  std::cout << std::endl;
  std::cout << utils::BOLD << "CONFIG SUBCOMMANDS:" << utils::RESET
            << std::endl;
  std::cout << "  " << utils::YELLOW << "config show" << utils::RESET
            << "                 Show current config" << std::endl;
  std::cout << "  " << utils::YELLOW << "config import <file>" << utils::RESET
            << "        Import from JSON file" << std::endl;
  std::cout << "  " << utils::YELLOW << "config set <key> [value]" << utils::RESET
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
  std::cout << utils::BOLD << "SERVICE SUBCOMMANDS:" << utils::RESET << std::endl;
  std::cout << "  " << utils::YELLOW << "service install" << utils::RESET
            << "            Install "
#ifdef __APPLE__
            << "launchd"
#elif defined(_WIN32)
            << "Windows service"
#else
            << "systemd"
#endif
            << " helper "
#ifdef _WIN32
            << "(needs Administrator once)" << std::endl;
#else
            << "(needs sudo once)" << std::endl;
#endif
  std::cout << "  " << utils::YELLOW << "service uninstall" << utils::RESET
            << "          Remove "
#ifdef __APPLE__
            << "launchd"
#elif defined(_WIN32)
            << "Windows service"
#else
            << "systemd"
#endif
            << " helper" << std::endl;
  std::cout << "  " << utils::YELLOW << "service status" << utils::RESET
            << "             Show helper status" << std::endl;
  std::cout << std::endl;
  std::cout << utils::BOLD << "EXAMPLES:" << utils::RESET << std::endl;
#ifdef _WIN32
  std::cout << utils::DIM
            << "  exv service install                    # Install Windows helper once"
            << utils::RESET << std::endl;
#else
  std::cout << utils::DIM
            << "  sudo exv service install               # Install root helper once"
            << utils::RESET << std::endl;
#endif
  std::cout << "  exv                                    # Start VPN via helper"
            << std::endl
            << "  exv -rt 3                              # Retry reconnect 3 times"
            << std::endl
      << "  exv -rt                                # Retry reconnect forever"
      << std::endl
      << "  exv --webui                            # Start VPN + WebUI server"
      << std::endl
      << "  exv --webui --foreground               # Start VPN + WebUI (attached)"
      << std::endl
      << "  exv stop                               # Stop VPN" << std::endl
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

static int handle_service(const std::vector<std::string> &args) {
  if (args.size() <= 2 || args[2] == "status") {
    return helper::show_service_status();
  }

  std::string subcmd = args[2];
  if (subcmd == "install") {
    return helper::install_service(utils::get_executable_path());
  }
  if (subcmd == "uninstall") {
    return helper::uninstall_service();
  }

  utils::print_error("Unknown service subcommand: " + subcmd);
  utils::print_info("Available: install, uninstall, status");
  return 1;
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
      utils::print_error("Usage: exv config set <key> [value]");
      utils::print_info(
          "Keys: server, username, password, mtu, useragent, log_file, remember_password, disable_dtls");
      return 1;
    }
    Config cfg = config::load();
    std::string inline_value = (args.size() >= 5) ? args[4] : "";
    return config::set_value(cfg, args[3], inline_value) ? 0 : 1;
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
  utils::enable_windows_ansi();

  std::vector<std::string> raw_args;
  for (int i = 0; i < argc; ++i) {
    raw_args.emplace_back(argv[i]);
  }

  if (raw_args.size() > 1 && raw_args[1] == "__helper-daemon") {
    return helper::daemon_main();
  }
  if (raw_args.size() > 2 && raw_args[1] == "__helper-exec") {
    return helper::worker_main(raw_args[2]);
  }
#ifdef _WIN32
  if (raw_args.size() > 1 && raw_args[1] == "__vpn-supervisor") {
    return vpn::supervisor_main();
  }
#endif
  if (raw_args.size() > 2 && raw_args[1] == "desktop-rpc") {
    nlohmann::json payload = nlohmann::json::object();
    if (raw_args.size() > 3) {
      try {
        payload = nlohmann::json::parse(raw_args[3]);
      } catch (const std::exception &ex) {
        std::cout << nlohmann::json{{"ok", false},
                                    {"error", std::string("invalid JSON payload: ") + ex.what()}}
                         .dump()
                  << std::endl;
        return 1;
      }
    }

    nlohmann::json result = app_api::handle_action(raw_args[2], payload);
    std::cout << result.dump() << std::endl;
    if (result.is_object() && result.value("ok", true) == false) {
      return 1;
    }
    return 0;
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
    args.push_back("start");
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

    // Warn if WebUI is requested but helper daemon is not installed
    bool want_webui = parsed.webui || (parsed.foreground && cfg.webui_enabled);
    if (want_webui && !helper::is_available()) {
#ifdef _WIN32
      utils::print_warning(
          "WebUI is requested but helper daemon is not installed. "
          "WebUI will have limited VPN control. "
          "The desktop app is the recommended interface for macOS and Windows. "
          "Install the helper with: exv service install from an elevated prompt");
#else
      utils::print_warning(
          "WebUI is requested but helper daemon is not installed. "
          "WebUI will have limited VPN control. "
          "The desktop app is the recommended interface for macOS and Windows. "
          "Install the helper with: sudo exv service install");
#endif
    }

    int vpn_result = vpn::start(cfg, parsed.retry_limit);

    if (vpn_result != 0) {
      return vpn_result;
    }

    // Default behavior: start VPN and return.
    // WebUI only starts when explicitly requested via --webui,
    // or when --foreground is given AND webui_enabled is true in config.
    if (want_webui) {
      if (parsed.foreground) {
        // Foreground mode: start WebUI in current process, block until Ctrl+C
        std::string config_dir = utils::get_config_dir();
        std::string log_path = utils::expand_home(cfg.log_file);

        config::ConfigManager config_mgr(config_dir);

        sse::SseBroadcaster log_broadcaster(
            log_path,
            []() -> std::string {
              return "{}";
            },
            8);

        sse::SseBroadcaster status_broadcaster(
            log_path,
            []() -> std::string {
#ifndef _WIN32
              int fd = socket(AF_UNIX, SOCK_STREAM, 0);
              if (fd < 0) return "";

              struct sockaddr_un addr {};
              addr.sun_family = AF_UNIX;
              snprintf(addr.sun_path, sizeof(addr.sun_path), "/var/run/exv-helper.sock");

              if (connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
                close(fd);
                return "";
              }

              std::string req = R"({"action":"status"})" "\n";
              if (write(fd, req.data(), req.size()) != static_cast<ssize_t>(req.size())) {
                close(fd);
                return "";
              }
              shutdown(fd, SHUT_WR);

              std::string raw;
              char buf[1024];
              ssize_t n;
              while ((n = read(fd, buf, sizeof(buf))) > 0) {
                raw.append(buf, static_cast<size_t>(n));
                if (raw.find('\n') != std::string::npos) break;
              }
              close(fd);
#else
              // Windows: use Named Pipe
              HANDLE hPipe = CreateFileA("\\\\.\\pipe\\exv-helper",
                                         GENERIC_READ | GENERIC_WRITE, 0, NULL,
                                         OPEN_EXISTING, 0, NULL);
              if (hPipe == INVALID_HANDLE_VALUE) return "";

              std::string req = R"({"action":"status"})" "\n";
              DWORD bytesWritten = 0;
              WriteFile(hPipe, req.c_str(), static_cast<DWORD>(req.size()), &bytesWritten, NULL);

              std::string raw;
              char buf[1024];
              DWORD bytesRead = 0;
              while (ReadFile(hPipe, buf, sizeof(buf), &bytesRead, NULL) && bytesRead > 0) {
                raw.append(buf, bytesRead);
                if (raw.find('\n') != std::string::npos) break;
              }
              CloseHandle(hPipe);
#endif
              auto nl = raw.find('\n');
              if (nl != std::string::npos) raw.resize(nl);
              raw = utils::trim(raw);
              if (raw.empty()) return "";

              try {
                auto resp = nlohmann::json::parse(raw);
                nlohmann::json status;
                status["connected"] = resp.value("running", false);
                status["network_ready"] = resp.value("network_ready", false);
                status["interface"] = resp.value("interface", "");
                status["internal_ip"] = resp.value("internal_ip", "");
                status["pid"] = resp.value("pid", -1);
                status["server"] = resp.value("server", "");
                virtual_network::add_status_fields(
                    status, status.value("interface", std::string()));
                return sse::format_sse_event("status", status.dump());
              } catch (...) {
                return "";
              }
            },
            8);

        log_broadcaster.start();
        status_broadcaster.start();

        webui::WebUIServer webui(config_mgr, log_broadcaster, status_broadcaster,
                                 cfg.webui_port, cfg.webui_bind);

        g_webui_server = &webui;
        signal(SIGINT, webui_signal_handler);
        signal(SIGTERM, webui_signal_handler);

        webui.start();

        std::cout << std::endl;
        utils::print_info("WebUI running in foreground (compatibility mode). Press Ctrl+C to stop.");
        while (!webui_stop_requested) {
#ifndef _WIN32
          pause();
#else
          Sleep(1000);
#endif
        }
        std::cout << std::endl;
        utils::print_info("Shutting down WebUI...");

        webui.stop();
        status_broadcaster.stop();
        log_broadcaster.stop();
        g_webui_server = nullptr;
      } else {
#ifndef _WIN32
        // Background mode: fork first, then start WebUI in child only.
        // This avoids the fork-after-thread bug where fork() only duplicates
        // the calling thread, leaving the HTTP server thread dead in the child.
        pid_t bg_pid = fork();
        if (bg_pid > 0) {
          // Parent: return to shell immediately
          std::cout << std::endl;
          utils::print_info("WebUI running in background (compatibility mode) at http://" +
                            cfg.webui_bind + ":" + std::to_string(cfg.webui_port) + "/");
          utils::print_info("Stop with: exv stop");
          return 0;
        }
        if (bg_pid < 0) {
          // Fork failed: fall back to foreground
          utils::print_warning("Could not detach to background, running in foreground.");
          std::string config_dir = utils::get_config_dir();
          std::string log_path = utils::expand_home(cfg.log_file);
          config::ConfigManager config_mgr(config_dir);
          sse::SseBroadcaster log_broadcaster(log_path, []() -> std::string { return "{}"; }, 8);
          sse::SseBroadcaster status_broadcaster(log_path, []() -> std::string { return ""; }, 8);
          log_broadcaster.start();
          status_broadcaster.start();
          webui::WebUIServer webui(config_mgr, log_broadcaster, status_broadcaster,
                                   cfg.webui_port, cfg.webui_bind);
          g_webui_server = &webui;
          signal(SIGINT, webui_signal_handler);
          signal(SIGTERM, webui_signal_handler);
          webui.start();
          while (!webui_stop_requested) { pause(); }
          webui.stop();
          status_broadcaster.stop();
          log_broadcaster.stop();
          g_webui_server = nullptr;
          return 0;
        }

        // Child: detach from terminal, then start WebUI
        setsid();
        (void)freopen("/dev/null", "r", stdin);
        (void)freopen("/dev/null", "w", stdout);
        (void)freopen("/dev/null", "w", stderr);

        std::string config_dir = utils::get_config_dir();
        std::string log_path = utils::expand_home(cfg.log_file);

        config::ConfigManager config_mgr(config_dir);

        sse::SseBroadcaster log_broadcaster(
            log_path,
            []() -> std::string {
              return "{}";
            },
            8);

        sse::SseBroadcaster status_broadcaster(
            log_path,
            []() -> std::string {
              int fd = socket(AF_UNIX, SOCK_STREAM, 0);
              if (fd < 0) return "";

              struct sockaddr_un addr {};
              addr.sun_family = AF_UNIX;
              snprintf(addr.sun_path, sizeof(addr.sun_path), "/var/run/exv-helper.sock");

              if (connect(fd, reinterpret_cast<sockaddr*>(&addr), sizeof(addr)) != 0) {
                close(fd);
                return "";
              }

              std::string req = R"({"action":"status"})" "\n";
              if (write(fd, req.data(), req.size()) != static_cast<ssize_t>(req.size())) {
                close(fd);
                return "";
              }
              shutdown(fd, SHUT_WR);

              std::string raw;
              char buf[1024];
              ssize_t n;
              while ((n = read(fd, buf, sizeof(buf))) > 0) {
                raw.append(buf, static_cast<size_t>(n));
                if (raw.find('\n') != std::string::npos) break;
              }
              close(fd);

              auto nl = raw.find('\n');
              if (nl != std::string::npos) raw.resize(nl);
              raw = utils::trim(raw);
              if (raw.empty()) return "";

              try {
                auto resp = nlohmann::json::parse(raw);
                nlohmann::json status;
                status["connected"] = resp.value("running", false);
                status["network_ready"] = resp.value("network_ready", false);
                status["interface"] = resp.value("interface", "");
                status["internal_ip"] = resp.value("internal_ip", "");
                status["pid"] = resp.value("pid", -1);
                status["server"] = resp.value("server", "");
                return sse::format_sse_event("status", status.dump());
              } catch (...) {
                return "";
              }
            },
            8);

        log_broadcaster.start();
        status_broadcaster.start();

        webui::WebUIServer webui(config_mgr, log_broadcaster, status_broadcaster,
                                 cfg.webui_port, cfg.webui_bind);

        g_webui_server = &webui;
        signal(SIGINT, webui_signal_handler);
        signal(SIGTERM, webui_signal_handler);

        webui.start();

        while (!webui_stop_requested) {
          pause();
        }

        webui.stop();
        status_broadcaster.stop();
        log_broadcaster.stop();
        g_webui_server = nullptr;
        return 0;
#else
        // Windows: no fork/setsid. When --webui is given without --foreground,
        // start WebUI in foreground as a compatibility fallback.
        std::string config_dir = utils::get_config_dir();
        std::string log_path = utils::expand_home(cfg.log_file);
        config::ConfigManager config_mgr(config_dir);
        sse::SseBroadcaster log_broadcaster(log_path, []() -> std::string { return "{}"; }, 8);
        sse::SseBroadcaster status_broadcaster(log_path, []() -> std::string { return ""; }, 8);
        log_broadcaster.start();
        status_broadcaster.start();
        webui::WebUIServer webui(config_mgr, log_broadcaster, status_broadcaster,
                                 cfg.webui_port, cfg.webui_bind);
        g_webui_server = &webui;
        signal(SIGINT, webui_signal_handler);
        signal(SIGTERM, webui_signal_handler);
        webui.start();
        std::cout << std::endl;
        utils::print_info("WebUI running in foreground (compatibility mode). Press Ctrl+C to stop.");
        while (!webui_stop_requested) {
          Sleep(1000);
        }
        std::cout << std::endl;
        utils::print_info("Shutting down WebUI...");
        webui.stop();
        status_broadcaster.stop();
        log_broadcaster.stop();
        g_webui_server = nullptr;
        return 0;
#endif
      }
    }

    // Default: VPN started, print status and return to shell.
    return 0;
  }
  if (cmd == "stop" || cmd == "-s") {
    return vpn::stop();
  }
  if (cmd == "status" || cmd == "-t") {
    return vpn::status();
  }
  if (cmd == "config" || cmd == "-c") {
    logger::init();
    return handle_config(args);
  }
  if (cmd == "service") {
    return handle_service(args);
  }
  if (cmd == "logs" || cmd == "-l") {
    logger::init();
    logger::show_logs();
    return 0;
  }

  utils::print_error("Unknown command: " + cmd);
  utils::print_info("Run 'exv help' for usage information.");
  return 1;
}
