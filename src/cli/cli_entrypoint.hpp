#pragma once

#include <string>
#include <vector>

namespace exv::cli {

struct CliEntrypointOptions {
  bool allow_self_core_candidate = false;
  bool interactive = true;
};

int run_cli_entrypoint(const std::vector<std::string> &args,
                       CliEntrypointOptions options = {});
int run_cli_entrypoint(int argc, char *argv[],
                       CliEntrypointOptions options = {});

} // namespace exv::cli
