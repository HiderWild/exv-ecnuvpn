#pragma once
#include <string>
#include <vector>

namespace exv::core {

// Entry point for `exv --mode=core` long-running process.
// Reads JSON-RPC requests from stdin, dispatches them through AppRpcDispatcher,
// and writes responses to stdout. Status change events are pushed as JSON lines.
//
// Returns 0 on clean shutdown (SIGTERM/SIGINT), non-zero on fatal error.
int core_process_main(const std::string& config_dir,
                      const std::string& home_dir);

} // namespace exv::core
