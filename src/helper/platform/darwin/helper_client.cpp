#include "helper/platform/helper_client.hpp"
#include "helper/platform/helper_platform.hpp"

#include <cstdio>
#include <string>

#include <fcntl.h>
#include <poll.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

namespace ecnuvpn {
namespace platform {
namespace {

std::string trim_copy(const std::string &value) {
  const auto first = value.find_first_not_of(" \t\r\n");
  if (first == std::string::npos)
    return "";
  const auto last = value.find_last_not_of(" \t\r\n");
  return value.substr(first, last - first + 1);
}

} // namespace

nlohmann::json send_helper_request(const nlohmann::json &request) {
  return send_helper_request(HelperEndpoint{helper_platform_config().endpoint},
                             request);
}

nlohmann::json send_helper_request(const HelperEndpoint &endpoint,
                                   const nlohmann::json &request) {
  std::string payload = request.dump();
  payload.push_back('\n');
  std::string raw;

  int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0) {
    return nlohmann::json{{"ok", false},
                          {"message", "Failed to create socket"}};
  }

  sockaddr_un addr {};
  addr.sun_family = AF_UNIX;
  std::snprintf(addr.sun_path, sizeof(addr.sun_path), "%s",
                endpoint.endpoint.c_str());

  // Non-blocking connect with 2-second timeout to avoid indefinite hangs
  // when the helper socket is stale or unreachable.
  int flags = fcntl(fd, F_GETFL, 0);
  fcntl(fd, F_SETFL, flags | O_NONBLOCK);

  int rc = ::connect(fd, reinterpret_cast<sockaddr *>(&addr), sizeof(addr));
  if (rc != 0 && errno != EINPROGRESS) {
    close(fd);
    return nlohmann::json{{"ok", false},
                          {"message", "Helper daemon not available"},
                          {"code", kHelperUnavailableCode}};
  }
  if (rc != 0) {
    struct pollfd pfd{};
    pfd.fd = fd;
    pfd.events = POLLOUT;
    rc = poll(&pfd, 1, 2000);
    if (rc <= 0) {
      close(fd);
      return nlohmann::json{{"ok", false},
                            {"message", "Helper daemon not available"},
                            {"code", kHelperUnavailableCode}};
    }
    int sock_err = 0;
    socklen_t len = sizeof(sock_err);
    getsockopt(fd, SOL_SOCKET, SO_ERROR, &sock_err, &len);
    if (sock_err != 0) {
      close(fd);
      return nlohmann::json{{"ok", false},
                            {"message", "Helper daemon not available"},
                            {"code", kHelperUnavailableCode}};
    }
  }

  // Restore blocking mode for subsequent read/write
  fcntl(fd, F_SETFL, flags);

  if (write(fd, payload.data(), payload.size()) !=
      static_cast<ssize_t>(payload.size())) {
    close(fd);
    return nlohmann::json{{"ok", false},
                          {"message", "Failed to send helper request"}};
  }
  shutdown(fd, SHUT_WR);

  char buffer[1024];
  ssize_t n = 0;
  while ((n = read(fd, buffer, sizeof(buffer))) > 0) {
    raw.append(buffer, static_cast<size_t>(n));
    if (raw.find('\n') != std::string::npos)
      break;
  }
  close(fd);

  auto nl = raw.find('\n');
  if (nl != std::string::npos)
    raw.resize(nl);
  raw = trim_copy(raw);

  if (raw.empty()) {
    return nlohmann::json{{"ok", false},
                          {"message", "Empty helper response"}};
  }

  try {
    return nlohmann::json::parse(raw);
  } catch (...) {
    return nlohmann::json{{"ok", false},
                          {"message", "Failed to parse helper response"}};
  }
}

} // namespace platform
} // namespace ecnuvpn
