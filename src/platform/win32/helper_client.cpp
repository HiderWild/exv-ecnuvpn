#include "platform/common/helper_client.hpp"
#include "platform/common/helper_platform.hpp"

#include <string>

#include <windows.h>

namespace exv {
namespace platform {
namespace {

constexpr DWORD kHelperResponseTimeoutMs = 10000;

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

  HANDLE hPipe = INVALID_HANDLE_VALUE;
  DWORD last_error = ERROR_SUCCESS;
  const DWORD start_tick = GetTickCount();
  const DWORD busy_deadline = start_tick + 30000;
  const DWORD missing_deadline = start_tick + 1000;
  while (GetTickCount() < busy_deadline) {
    hPipe = CreateFileA(endpoint.endpoint.c_str(),
                        GENERIC_READ | GENERIC_WRITE, 0, NULL,
                        OPEN_EXISTING, 0, NULL);
    if (hPipe != INVALID_HANDLE_VALUE)
      break;

    last_error = GetLastError();
    if (last_error != ERROR_PIPE_BUSY && last_error != ERROR_FILE_NOT_FOUND) {
      break;
    }

    if (last_error == ERROR_PIPE_BUSY) {
      WaitNamedPipeA(endpoint.endpoint.c_str(), 250);
    } else {
      if (GetTickCount() >= missing_deadline)
        break;
      Sleep(100);
    }
  }

  if (hPipe == INVALID_HANDLE_VALUE) {
    return nlohmann::json{{"ok", false},
                          {"message", "Helper daemon not available"},
                          {"code", kHelperUnavailableCode},
                          {"win32_error", static_cast<int>(last_error)}};
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
  const ULONGLONG read_deadline = GetTickCount64() + kHelperResponseTimeoutMs;
  while (GetTickCount64() < read_deadline) {
    DWORD available = 0;
    if (!PeekNamedPipe(hPipe, NULL, 0, NULL, &available, NULL)) {
      break;
    }
    if (available == 0) {
      Sleep(10);
      continue;
    }
    if (!ReadFile(hPipe, buffer, sizeof(buffer), &bytesRead, NULL) ||
        bytesRead == 0) {
      break;
    }
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
} // namespace exv
