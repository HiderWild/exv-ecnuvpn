#include "platform/common/helper_service_manager.hpp"

#include "platform/common/helper_lifecycle.hpp"
#include "platform/common/helper_platform.hpp"
#include "utils.hpp"

#include <sys/stat.h>

#include <fstream>
#include <iostream>
#include <sstream>
#include <string>

namespace ecnuvpn {
namespace platform {
namespace {

bool wait_until_ready(const HelperServiceManagerContext &context, int attempts,
                     unsigned int delay_us) {
  return context.wait_until_available &&
         context.wait_until_available(attempts, delay_us);
}

bool send_helper_request(const HelperServiceManagerContext &context,
                         const nlohmann::json &request,
                         nlohmann::json *response,
                         std::string *error_message,
                         int timeout_seconds = 15) {
  return context.send_request &&
         context.send_request(request, response, error_message,
                              timeout_seconds);
}

void print_runtime_status_if_available(const HelperServiceManagerContext &context,
                                       bool available) {
  if (!available)
    return;

  nlohmann::json response;
  std::string error_message;
  if (send_helper_request(context, nlohmann::json{{"action", "status"}},
                          &response, &error_message) &&
      response.value("ok", false)) {
    std::cout << "  VPN Running     : "
              << (response.value("running", false) ? "yes" : "no")
              << std::endl;
    if (response.value("running", false)) {
      std::cout << "  Session Owner   : "
                << response.value("owner_username", std::string())
                << std::endl;
    }
  }
}

} // namespace

int install_helper_service(const std::string &executable_path,
                           const HelperServiceManagerContext &context) {
  const auto &platform_config = helper_platform_config();

  if (!utils::check_root()) {
    utils::print_error("Root privileges required. Please run with sudo.");
    return 1;
  }

  std::string exec_path = executable_path.empty() ? utils::get_executable_path()
                                                  : executable_path;
  if (exec_path.empty()) {
    utils::print_error("Failed to resolve the exv executable path.");
    return 1;
  }

  if (exec_path != platform_config.stable_install_path) {
    return platform::copy_self_to_stable_path_and_reexec(exec_path);
  }

  std::string shell_command =
      "if [ ! -x " + utils::shell_quote(exec_path) +
      " ]; then exit 0; fi; exec " + utils::shell_quote(exec_path) +
      " __helper-daemon";

  std::ostringstream plist;
  plist << "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n";
  plist << "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" "
           "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n";
  plist << "<plist version=\"1.0\">\n";
  plist << "<dict>\n";
  plist << "  <key>Label</key>\n";
  plist << "  <string>" << platform_config.service_label << "</string>\n";
  plist << "  <key>ProgramArguments</key>\n";
  plist << "  <array>\n";
  plist << "    <string>/bin/sh</string>\n";
  plist << "    <string>-c</string>\n";
  plist << "    <string>" << shell_command << "</string>\n";
  plist << "  </array>\n";
  plist << "  <key>RunAtLoad</key>\n";
  plist << "  <true/>\n";
  plist << "  <key>KeepAlive</key>\n";
  plist << "  <dict>\n";
  plist << "    <key>SuccessfulExit</key>\n";
  plist << "    <false/>\n";
  plist << "  </dict>\n";
  plist << "</dict>\n";
  plist << "</plist>\n";

  std::ofstream ofs(platform_config.service_definition_path);
  if (!ofs.is_open()) {
    utils::print_error("Failed to write LaunchDaemon plist: " +
                       std::string(platform_config.service_definition_path));
    return 1;
  }
  ofs << plist.str();
  ofs.close();
  chmod(platform_config.service_definition_path, 0644);

  utils::run_command(std::string("launchctl bootout system ") +
                     platform_config.service_definition_path +
                     " >/dev/null 2>&1");
  if (utils::run_command(std::string("launchctl bootstrap system ") +
                         platform_config.service_definition_path) != 0) {
    utils::print_error("Failed to bootstrap EXV helper LaunchDaemon.");
    return 1;
  }

  bool helper_ready = wait_until_ready(context, 50, 100000);

  platform::fix_config_dir_ownership();

  utils::print_success("EXV helper service installed.");
  if (!helper_ready) {
    utils::print_warning(
        "Helper service was installed, but it has not responded on the socket yet.");
    utils::print_info("Run 'exv service status' again in a moment if needed.");
  }
  utils::print_info("You can now run 'exv' and 'exv stop' without sudo.");
  return 0;
}

int uninstall_helper_service(const HelperServiceManagerContext &context) {
  const auto &platform_config = helper_platform_config();

  if (!utils::check_root()) {
    utils::print_error("Root privileges required. Please run with sudo.");
    return 1;
  }

  nlohmann::json response;
  std::string error_message;
  send_helper_request(context, nlohmann::json{{"action", "stop"}}, &response,
                      &error_message);

  platform::cleanup_routes();
  platform::kill_all_supervisors();

  utils::run_command(std::string("launchctl bootout system ") +
                     platform_config.service_definition_path +
                     " >/dev/null 2>&1");
  if (*platform_config.service_definition_path)
    std::remove(platform_config.service_definition_path);
  if (*platform_config.endpoint)
    std::remove(platform_config.endpoint);
  if (context.clear_session_state)
    context.clear_session_state();

  utils::print_success("EXV helper service uninstalled.");
  return 0;
}

int show_helper_service_status(const HelperServiceManagerContext &context) {
  const auto &platform_config = helper_platform_config();

  utils::print_header("EXV Service Status");

  bool installed = utils::file_exists(platform_config.service_definition_path);
  bool available = installed ? wait_until_ready(context, 10, 100000) : false;
  std::cout << "  Installed       : " << (installed ? "yes" : "no")
            << std::endl;
  std::cout << "  Socket Ready    : " << (available ? "yes" : "no")
            << std::endl;
  print_runtime_status_if_available(context, available);
  std::cout << std::endl;
  return 0;
}

} // namespace platform
} // namespace ecnuvpn