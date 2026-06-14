#include "helper/helper_ipc.hpp"

#include <cerrno>
#include <cstring>
#include <cstdio>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/stat.h>
#include <sys/un.h>
#include <unistd.h>

namespace ecnuvpn {
namespace helper {

class LinuxIpcServer : public IpcServer {
  int server_fd_ = -1;
  int client_fd_ = -1;
  unsigned int peer_uid_ = 0;
  unsigned int peer_gid_ = 0;

public:
  ~LinuxIpcServer() override { close(); }

  bool start(const std::string &path) override {
    server_fd_ = socket(AF_UNIX, SOCK_STREAM, 0);
    if (server_fd_ < 0)
      return false;

    sockaddr_un addr {};
    addr.sun_family = AF_UNIX;
    std::snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", path.c_str());

    if (bind(server_fd_, reinterpret_cast<sockaddr *>(&addr), sizeof(addr)) != 0) {
      ::close(server_fd_);
      server_fd_ = -1;
      return false;
    }

    // Linux: world-readable socket (any local user can manage VPN)
    chmod(path.c_str(), 0666);

    if (listen(server_fd_, 8) != 0) {
      ::close(server_fd_);
      server_fd_ = -1;
      std::remove(path.c_str());
      return false;
    }
    return true;
  }

  bool accept_client() override {
    client_fd_ = ::accept(server_fd_, nullptr, nullptr);
    if (client_fd_ < 0)
      return false;

    int flags = fcntl(client_fd_, F_GETFD);
    if (flags >= 0)
      fcntl(client_fd_, F_SETFD, flags | FD_CLOEXEC);

    return true;
  }

  bool verify_client() override {
    struct ucred cred;
    socklen_t cred_len = sizeof(cred);
    if (getsockopt(client_fd_, SOL_SOCKET, SO_PEERCRED, &cred, &cred_len) != 0) {
      ::close(client_fd_);
      client_fd_ = -1;
      return false;
    }
    peer_uid_ = cred.uid;
    peer_gid_ = cred.gid;
    return true;
  }

  std::string read_request() override {
    std::string raw;
    char buffer[1024];
    ssize_t n = 0;
    // Read until newline (not EOF) to support persistent connections.
    while ((n = read(client_fd_, buffer, sizeof(buffer))) > 0) {
      raw.append(buffer, buffer + n);
      if (raw.find('\n') != std::string::npos)
        break;
    }
    return raw;
  }

  bool send_response(const std::string &response) override {
    std::string payload = response;
    payload.push_back('\n');
    ssize_t written = write(client_fd_, payload.data(), payload.size());
    // Do NOT close client_fd_ here -- keep connection open for persistent IPC.
    // The caller is responsible for calling close_client() when done.
    return written == static_cast<ssize_t>(payload.size());
  }

  void close_client() override {
    if (client_fd_ >= 0) {
      ::close(client_fd_);
      client_fd_ = -1;
    }
  }

  void close_server() override {
    if (server_fd_ >= 0) {
      ::close(server_fd_);
      server_fd_ = -1;
    }
  }

  void close() override {
    close_client();
    close_server();
  }

  int server_fd() const override { return server_fd_; }

  unsigned int peer_uid() const override { return peer_uid_; }
  unsigned int peer_gid() const override { return peer_gid_; }
};

std::unique_ptr<IpcServer> create_ipc_server() {
  return std::make_unique<LinuxIpcServer>();
}

} // namespace helper
} // namespace ecnuvpn
