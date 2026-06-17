#pragma once

#include <functional>
#include <iosfwd>
#include <string>
#include <vector>

namespace exv::cli {

struct CliCoreResolution {
  bool ok = false;
  std::string ipc_path;
  std::string message;
};

struct CliCommandDeps {
  std::function<CliCoreResolution()> resolve_core;
  std::function<std::string(const std::string &ipc_path,
                            const std::string &request_line)>
      send_core_request;
  std::function<bool(const std::string &prompt)> confirm;
  std::function<std::string()> version_probe;
  std::ostream *out = nullptr;
  std::ostream *err = nullptr;
  bool interactive = true;
};

int run_cli_command(const std::vector<std::string> &args, CliCommandDeps deps);

} // namespace exv::cli
