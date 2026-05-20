#pragma once

#include <string>

namespace nlohmann {
class json;
}

namespace ecnuvpn {
namespace platform {

// Send a JSON request to the helper daemon and receive a JSON response.
// Returns true on success; on failure, error_message is populated.
bool send_helper_request(const nlohmann::json &request,
                         nlohmann::json *response,
                         std::string *error_message = nullptr,
                         int timeout_ms = 15000);

// Poll the helper daemon until it responds or attempts are exhausted.
// delay_us is the microsecond delay between attempts (converted to ms on Windows).
bool wait_for_helper_available(int attempts = 1, unsigned int delay_us = 0);

} // namespace platform
} // namespace ecnuvpn
