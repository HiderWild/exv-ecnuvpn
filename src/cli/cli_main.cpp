#include "cli/cli_commands.hpp"

#include "cli/console.hpp"
#include "cli/pipe_client.hpp"
#include "platform/common/core_resolver.hpp"
#include "platform/common/core_resolver_platform_deps.hpp"
#include "platform/common/process_utils.hpp"
#include "runtime/runtime_context.hpp"

#include <iostream>
#include <string>
#include <vector>

namespace {

exv::cli::CliCoreResolution resolve_core_for_cli() {
  auto deps = exv::core::lifecycle::make_platform_core_resolver_deps();
  auto result = exv::core::lifecycle::resolve_core({}, deps);

  exv::cli::CliCoreResolution resolution;
  resolution.ok = result.status == exv::core::lifecycle::CoreResolveStatus::ReuseExisting ||
                  result.status == exv::core::lifecycle::CoreResolveStatus::LaunchRequired;
  resolution.ipc_path = result.ipc_path;
  resolution.message = result.message;
  return resolution;
}

std::string send_core_request(const std::string &ipc_path,
                              const std::string &request_line) {
  exv::cli::PipeClient client;
  if (!client.connect(ipc_path)) {
    return {};
  }
  std::string response = client.send_request(request_line);
  client.disconnect();
  return response;
}

bool confirm_prompt(const std::string &prompt) {
  std::cout << prompt << " Type 'yes' to continue: ";
  std::string answer;
  std::getline(std::cin, answer);
  return answer == "yes";
}

std::string core_version_probe() {
  auto deps = exv::core::lifecycle::make_platform_core_resolver_deps();
  const std::string frontend_path = ecnuvpn::platform::get_executable_path();
  const char *env_core_path = std::getenv("EXV_CORE_PATH");
  const char *path = std::getenv("PATH");
  auto candidate = exv::core::lifecycle::find_core_candidate(
      frontend_path,
      env_core_path ? std::string(env_core_path) : std::string(),
      path ? std::string(path) : std::string());
  if (!candidate.has_value()) {
    return "core_not_found\n";
  }
  return ecnuvpn::platform::run_command_output(
      ecnuvpn::platform::shell_quote(*candidate) + " --version");
}

} // namespace

int main(int argc, char *argv[]) {
  ecnuvpn::cli::enable_windows_ansi();
  ecnuvpn::runtime::bootstrap();

  std::vector<std::string> args;
  for (int i = 0; i < argc; ++i) {
    args.emplace_back(argv[i]);
  }

  exv::cli::CliCommandDeps deps;
  deps.resolve_core = resolve_core_for_cli;
  deps.send_core_request = send_core_request;
  deps.confirm = confirm_prompt;
  deps.version_probe = core_version_probe;
  deps.out = &std::cout;
  deps.err = &std::cerr;
  deps.interactive = true;
  return exv::cli::run_cli_command(args, deps);
}
