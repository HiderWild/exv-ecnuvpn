#include "platform/common/helper_client.hpp"
#include "platform/common/helper_platform.hpp"

#include "logger.hpp"
#include "utils.hpp"

#include <cerrno>
#include <cstring>
#include <cstdio>
#include <fcntl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>
#include <sys/select.h>

#include <nlohmann/json.hpp>

namespace ecnuvpn {
namespace platform {

namespace {

bool connect_helper_socket(int *out_fd) {
  if (!out_fd)
    return false;

  std::string endpoint = helper_endpoint_path();

  int fd = socket(AF_UNIX, SOCK_STREAM, 0);
  if (fd < 0)
    return false;

  sockaddr_un addr {};
  addr.sun_family = AF_UNIX;
  std::snprintf(addr.sun_path, sizeof(addr.sun_path), "%s", endpoint.c_str());

  if (connect(fd, reinterpret_cast<sockaddr *>(&addr), sizeof(addr)) != 0) {
    close(fd);
    return false;
  }

  *out_fd = fd;
  return true;
}

} // namespace

bool send_helper_request(const nlohmann::json &request,
                         nlohmann::json *response,
                         std::string *error_message,
                         int timeout_ms) {
  std::string raw;

  int fd = -1;
  if (!connect_helper_socket(&fd)) {
    if (error_message)
      *error_message = "EXV helper is not available.";
    return false;
  }

  std::string payload = request.dump();
  payload.push_back('\n');
  if (write(fd, payload.data(), payload.size()) !=
      static_cast<ssize_t>(payload.size())) {
    if (error_message)
      *error_message = "Failed to send request to EXV helper.";
    close(fd);
    return false;
  }
  shutdown(fd, SHUT_WR);

  char buffer[1024];
  ssize_t n = 0;

  struct timeval tv;
  tv.tv_sec = timeout_ms / 1000;
  tv.tv_usec = (timeout_ms % 1000) * 1000;

  fd_set readfds;
  FD_ZERO(&readfds);
  FD_SET(fd, &readfds);

  int sel_ret = select(fd + 1, &readfds, nullptr, nullptr, &tv);
  if (sel_ret <= 0) {
    if (error_message) {
      if (sel_ret == 0)
        *error_message = "EXV helper request timed out.";
      else
        *error_message = "EXV helper select error.";
    }
    close(fd);
    return false;
  }

  while ((n = read(fd, buffer, sizeof(buffer))) > 0) {
    raw.append(buffer, buffer + n);
    if (raw.find('\n') != std::string::npos)
      break;
  }
  close(fd);

  std::size_t newline_pos = raw.find('\n');
  if (newline_pos != std::string::npos)
    raw.resize(newline_pos);
  raw = utils::trim(raw);

  if (raw.empty()) {
    if (error_message)
      *error_message = "EXV helper returned an empty response.";
    return false;
  }

  try {
    if (response)
      *response = nlohmann::json::parse(raw);
    return true;
  } catch (...) {
    if (error_message)
      *error_message = "Failed to parse EXV helper response.";
    return false;
  }
}

bool wait_for_helper_available(int attempts, unsigned int delay_us) {
  for (int i = 0; i < attempts; ++i) {
    nlohmann::json response;
    std::string error_message;
    bool ok = send_helper_request(nlohmann::json{{"action", "status"}},
                                  &response, &error_message);
    if (ok)
      return true;
    if (i + 1 < attempts && delay_us > 0)
      usleep(delay_us);
  }
  return false;
}

} // namespace platform
} // namespace ecnuvpn
