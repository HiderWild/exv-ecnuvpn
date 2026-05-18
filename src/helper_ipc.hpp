#pragma once

#include <memory>
#include <string>

namespace ecnuvpn {
namespace helper {

class IpcServer {
public:
  virtual ~IpcServer() = default;

  // Start listening on the given path (socket path or pipe name)
  virtual bool start(const std::string &path) = 0;

  // Block until a client connects; returns true on success
  virtual bool accept_client() = 0;

  // Verify the connected client's credentials (uid/gid on POSIX, SID on Windows)
  // Returns true if the client is authorized
  virtual bool verify_client() = 0;

  // Read the full request from the current client (newline-delimited JSON)
  virtual std::string read_request() = 0;

  // Send a response to the current client
  virtual bool send_response(const std::string &response) = 0;

  // Close the current client connection (server keeps listening)
  virtual void close_client() {}

  // Close the server listen socket only (client fd stays open)
  // Used by forked children to stop accepting new connections while
  // still sending the response on the inherited client fd.
  virtual void close_server() {}

  // Close the server and release resources
  virtual void close() = 0;

  // Get the server listen fd (POSIX only; returns -1 on Windows)
  // Used by signal handlers to interrupt accept()
  virtual int server_fd() const { return -1; }

  // Get the peer uid/gid after verify_client (POSIX only; Windows returns 0)
  virtual unsigned int peer_uid() const = 0;
  virtual unsigned int peer_gid() const = 0;
};

// Factory: creates the platform-appropriate IpcServer
std::unique_ptr<IpcServer> create_ipc_server();

} // namespace helper
} // namespace ecnuvpn
