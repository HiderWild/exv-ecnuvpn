#include "cli/cli_entrypoint.hpp"

#include "cli/cli_commands.hpp"
#include "cli/console.hpp"
#include "cli/pipe_client.hpp"
#include "platform/common/core_resolver.hpp"
#include "platform/common/core_resolver_platform_deps.hpp"
#include "platform/common/process_utils.hpp"
#include "runtime/runtime_context.hpp"

#include <cstdlib>
#include <iostream>
#include <string>
#include <vector>

namespace exv::cli {
namespace {

CliEntrypointOptions current_options;

exv::core::lifecycle::CoreResolverDeps
make_cli_resolver_deps(const CliEntrypointOptions &options) {
  auto deps = exv::core::lifecycle::make_platform_core_resolver_deps();
  if (options.allow_self_core_candidate) {
    deps.get_frontend_executable_path = []() { return std::string(); };
  }
  return deps;
}

CliCoreResolution resolve_core_for_cli() {
  auto deps = make_cli_resolver_deps(current_options);
  exv::core::lifecycle::CoreResolveOptions options;
  if (current_options.allow_self_core_candidate) {
    options.preferred_core_path = exv::platform::get_executable_path();
  }
  auto result = exv::core::lifecycle::resolve_core(options, deps);

  CliCoreResolution resolution;
  resolution.ok =
      result.status == exv::core::lifecycle::CoreResolveStatus::ReuseExisting ||
      result.status == exv::core::lifecycle::CoreResolveStatus::LaunchRequired;
  resolution.ipc_path = result.ipc_path;
  resolution.message = result.message;
  return resolution;
}

std::string send_core_request(const std::string &ipc_path,
                              const std::string &request_line) {
  PipeClient client;
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
  const std::string frontend_path = current_options.allow_self_core_candidate
                                        ? std::string()
                                        : exv::platform::get_executable_path();
  const char *env_core_path = std::getenv("EXV_CORE_PATH");
  const char *path = std::getenv("PATH");
  auto candidate = exv::core::lifecycle::find_core_candidate(
      frontend_path, env_core_path ? std::string(env_core_path) : std::string(),
      path ? std::string(path) : std::string());
  if (!candidate.has_value() && current_options.allow_self_core_candidate) {
    const std::string self = exv::platform::get_executable_path();
    if (!self.empty()) {
      candidate = self;
    }
  }
  if (!candidate.has_value()) {
    return "core_not_found\n";
  }
  return exv::platform::run_command_output(
      exv::platform::shell_quote(*candidate) + " --version");
}

} // namespace

int run_cli_entrypoint(const std::vector<std::string> &args,
                       CliEntrypointOptions options) {
  enable_windows_ansi();
  exv::runtime::bootstrap();

  current_options = options;

  CliCommandDeps deps;
  deps.resolve_core = resolve_core_for_cli;
  deps.send_core_request = send_core_request;
  deps.confirm = confirm_prompt;
  deps.version_probe = core_version_probe;
  deps.out = &std::cout;
  deps.err = &std::cerr;
  deps.interactive = options.interactive;
  return run_cli_command(args, deps);
}

int run_cli_entrypoint(int argc, char *argv[], CliEntrypointOptions options) {
  std::vector<std::string> args;
  for (int i = 0; i < argc; ++i) {
    args.emplace_back(argv[i]);
  }
  return run_cli_entrypoint(args, options);
}

} // namespace exv::cli
