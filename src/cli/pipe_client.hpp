#pragma once

#include <string>

namespace exv::cli {

// Thin wrapper: connect to core pipe, send one JSON-RPC request, receive response, disconnect.
class PipeClient {
public:
  PipeClient() = default;
  ~PipeClient();

  PipeClient(const PipeClient&) = delete;
  PipeClient& operator=(const PipeClient&) = delete;

  // Connect to the core process pipe. Returns true on success.
  bool connect(const std::string& pipe_path);

  // Send a JSON-RPC request line and receive the response line.
  // Returns empty string on failure.
  std::string send_request(const std::string& request_line);

  // Disconnect from the pipe.
  void disconnect();

  // Check if connected.
  bool is_connected() const;

  // Probe whether the IPC endpoint is available without staying connected.
  // Connects and immediately disconnects; returns true if the connect succeeded.
  static bool probe(const std::string& pipe_path);

private:
  void* handle_ = nullptr; // platform-specific: HANDLE (Win) or int fd (Unix)
  bool connected_ = false;
};

} // namespace exv::cli
