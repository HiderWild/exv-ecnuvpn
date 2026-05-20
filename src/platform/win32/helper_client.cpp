#include "platform/common/helper_client.hpp"
#include "platform/common/helper_platform.hpp"

#include <string>

#include <windows.h>

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
  std::string payload = request.dump();
  payload.push_back('\n');
  std::string raw;
  const auto &config = helper_platform_config();

  HANDLE hPipe = CreateFileA(config.endpoint,
                             GENERIC_READ | GENERIC_WRITE, 0, NULL,
                             OPEN_EXISTING, 0, NULL);
  if (hPipe == INVALID_HANDLE_VALUE) {
    return nlohmann::json{{"ok", false},
                          {"message", "Helper daemon not available"},
                          {"code", kHelperUnavailableCode}};
  }

  DWORD bytesWritten = 0;
  if (!WriteFile(hPipe, payload.c_str(), static_cast<DWORD>(payload.size()),
                 &bytesWritten, NULL) ||
      bytesWritten != payload.size()) {
    CloseHandle(hPipe);
    return nlohmann::json{{"ok", false},
                          {"message", "Failed to send helper request"}};
  }

  char buffer[1024];
  DWORD bytesRead = 0;
  while (ReadFile(hPipe, buffer, sizeof(buffer), &bytesRead, NULL) &&
         bytesRead > 0) {
    raw.append(buffer, bytesRead);
    if (raw.find('\n') != std::string::npos)
      break;
  }
  CloseHandle(hPipe);

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