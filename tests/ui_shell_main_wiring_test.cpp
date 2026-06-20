#include <fstream>
#include <iostream>
#include <iterator>
#include <string>

#ifndef EXV_SOURCE_DIR
#error "EXV_SOURCE_DIR must be defined"
#endif

namespace {

std::string read_file(const std::string &path) {
  std::ifstream input(path);
  return {std::istreambuf_iterator<char>(input),
          std::istreambuf_iterator<char>()};
}

bool contains(const std::string &text, const std::string &needle) {
  return text.find(needle) != std::string::npos;
}

} // namespace

int main() {
  const std::string root = EXV_SOURCE_DIR;
  const std::string main_cpp =
      read_file(root + "/src/app/ui_shell/ui_shell_main.cpp");
  const std::string process_manager_cpp =
      read_file(root + "/src/app/ui_shell/core_process_manager.cpp");
  const std::string runtime_cpp =
      read_file(root + "/src/app/ui_shell/ui_shell_runtime.cpp");
  const std::string pipe_ipc_cpp =
      read_file(root + "/src/core/pipe_ipc.cpp");
  const std::string core_process_cpp =
      read_file(root + "/src/core/core_process.cpp");
  const std::string rpc_action_metadata_cpp =
      read_file(root + "/src/core/rpc/rpc_action_metadata.cpp");
  const std::string desktop_vpn_actions_cpp =
      read_file(root + "/src/core/app_api/desktop_vpn_actions.cpp");
  const std::string desktop_vpn_actions_hpp =
      read_file(root + "/src/core/app_api/desktop_vpn_actions.hpp");
  int failures = 0;

  if (!contains(main_cpp, "run_ui_shell_window(")) {
    std::cerr << "exv-ui main must run the neutral runtime\n";
    ++failures;
  }
  if (!contains(main_cpp, "create_core_process_transport(")) {
    std::cerr << "exv-ui main must create a production core RPC transport\n";
    ++failures;
  }
  if (!contains(main_cpp, "CoreProcessLaunch core_launch")) {
    std::cerr << "exv-ui main must build an explicit core launch descriptor\n";
    ++failures;
  }
  if (!contains(main_cpp, "options.exv_path") ||
      !contains(main_cpp, "config.state_dir") ||
      !contains(main_cpp, "exv::runtime::paths().home")) {
    std::cerr << "exv-ui main must pass packaged core and runtime paths into the managed core process\n";
    ++failures;
  }
  if (contains(main_cpp, "use_stdin = false")) {
    std::cerr << "exv-ui main must keep UI-managed core on stdin/stdout for push events\n";
    ++failures;
  }
  if (contains(main_cpp, "classify_core_state(")) {
    std::cerr << "exv-ui main must not prelaunch a daemon core before starting the stdin/stdout core\n";
    ++failures;
  }
  if (!contains(process_manager_cpp, "PipeCoreRpcTransport")) {
    std::cerr << "core process manager must provide a pipe transport for daemon core RPC\n";
    ++failures;
  }
  if (!contains(runtime_cpp, "set_event_handler") ||
      !contains(runtime_cpp, "pump_events") ||
      !contains(runtime_cpp, "window.emit_event")) {
    std::cerr << "exv-ui runtime must pump stdout core events into the renderer\n";
    ++failures;
  }
  if (!contains(runtime_cpp, "core.shutdown")) {
    std::cerr << "exv-ui runtime must ask daemon core to shut down on window exit\n";
    ++failures;
  }
  if (!contains(pipe_ipc_cpp, "ConvertStringSecurityDescriptorToSecurityDescriptorA") ||
      !contains(pipe_ipc_cpp, "PIPE_ACCESS_DUPLEX")) {
    std::cerr << "Windows daemon core pipe must set an explicit DACL for medium UI clients\n";
    ++failures;
  }
  if (contains(main_cpp,
               "return exv::platform::win32::ui_shell::"
               "run_webview2_host(config);")) {
    std::cerr << "exv-ui main must not bypass run_ui_shell_window on Windows\n";
    ++failures;
  }
  if (!contains(desktop_vpn_actions_hpp, "shutdown_desktop_vpn_runtime")) {
    std::cerr << "desktop VPN actions must expose a runtime shutdown hook\n";
    ++failures;
  }
  if (!contains(desktop_vpn_actions_cpp, "void shutdown_desktop_vpn_runtime()") ||
      !contains(desktop_vpn_actions_cpp, "g_desktop_connect_jobs.shutdown") ||
      !contains(desktop_vpn_actions_cpp, "reset_tunnel_controller()")) {
    std::cerr << "desktop VPN runtime shutdown must stop jobs and reset the controller\n";
    ++failures;
  }
  if (!contains(core_process_cpp, "shutdown_desktop_vpn_runtime()")) {
    std::cerr << "daemon core shutdown must release desktop VPN runtime resources\n";
    ++failures;
  }
  if (!contains(core_process_cpp, "is_core_owned_control_action") ||
      !contains(core_process_cpp, "core.shutdown")) {
    std::cerr << "core process must route desktop-envelope core.shutdown to the core dispatcher\n";
    ++failures;
  }
  if (!contains(rpc_action_metadata_cpp, "\"core.shutdown\"")) {
    std::cerr << "core.shutdown must use the control RPC lane\n";
    ++failures;
  }

  return failures == 0 ? 0 : 1;
}
