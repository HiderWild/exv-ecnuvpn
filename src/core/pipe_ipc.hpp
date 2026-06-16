#pragma once

#include <functional>
#include <memory>
#include <string>

namespace exv::core {

// Callback: receives a JSON-RPC request line, returns a JSON-RPC response line.
using PipeRequestHandler = std::function<std::string(const std::string& request_line)>;

// Cross-platform named pipe / Unix socket listener for CLI connections.
// On Windows: \\.\pipe\exv-core-ipc-v1
// On Unix:    {state_dir}/exv-core-ipc-v1.sock
class PipeIpcListener {
public:
  explicit PipeIpcListener(const std::string& pipe_path);
  ~PipeIpcListener();

  PipeIpcListener(const PipeIpcListener&) = delete;
  PipeIpcListener& operator=(const PipeIpcListener&) = delete;

  // Create/bind the pipe. Returns false if already in use (single-instance guard).
  bool start();

  // Stop listening and clean up.
  void stop();

  // Accept and process one pending connection (non-blocking).
  // Calls handler(request_line) → writes response_line back, then disconnects.
  // Returns true if a connection was processed.
  bool accept_one(PipeRequestHandler handler);

  // Native handle for select/poll integration.
  void* native_handle() const;

private:
  struct Impl;
  std::unique_ptr<Impl> impl_;
};

// Canonical pipe path for the current platform.
// Publicly kept for compatibility; internally this now delegates to the
// versioned lifecycle IPC path.
std::string core_pipe_path();

} // namespace exv::core
