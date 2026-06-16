#include "core/lifecycle/core_identity.hpp"

#include "contracts/generated/system_contract.hpp"
#include "platform/common/crypto_backend.hpp"
#include "platform/common/process_utils.hpp"

#include <array>
#include <chrono>
#include <cstdint>
#include <ctime>
#include <iomanip>
#include <sstream>
#include <string>

#ifdef _WIN32
#include <windows.h>
#else
#include <unistd.h>
#endif

#ifndef ECNUVPN_VERSION
#define ECNUVPN_VERSION "dev"
#endif

namespace exv::core::lifecycle {
namespace {

int current_process_id() {
#ifdef _WIN32
    return static_cast<int>(GetCurrentProcessId());
#else
    return static_cast<int>(getpid());
#endif
}

std::string bytes_to_hex(const uint8_t* data, std::size_t len) {
    std::ostringstream out;
    out << std::hex << std::setfill('0');
    for (std::size_t i = 0; i < len; ++i) {
        out << std::setw(2) << static_cast<unsigned int>(data[i]);
    }
    return out.str();
}

std::string render_started_at(
    const std::chrono::system_clock::time_point& timestamp) {
    const auto time = std::chrono::system_clock::to_time_t(timestamp);
    std::tm utc = {};
#ifdef _WIN32
    gmtime_s(&utc, &time);
#else
    gmtime_r(&time, &utc);
#endif

    const auto millis = std::chrono::duration_cast<std::chrono::milliseconds>(
                            timestamp.time_since_epoch()) %
                        1000;

    std::ostringstream out;
    out << std::put_time(&utc, "%Y-%m-%dT%H:%M:%S") << '.'
        << std::setw(3) << std::setfill('0') << millis.count() << 'Z';
    return out.str();
}

std::string ipc_protocol_version() {
    return "ipc-v" +
           std::to_string(exv::contracts::generated::IPC_PROTOCOL_MAJOR);
}

} // namespace

CoreIdentity make_core_identity() {
    const auto now = std::chrono::system_clock::now();
    const int pid = current_process_id();

    std::array<uint8_t, 8> random_bytes = {};
    std::string core_instance_id;
    if (ecnuvpn::platform::fill_random_bytes(random_bytes.data(),
                                             random_bytes.size())) {
        core_instance_id =
            "core-" + bytes_to_hex(random_bytes.data(), random_bytes.size());
    } else {
        const auto unix_ms =
            std::chrono::duration_cast<std::chrono::milliseconds>(
                now.time_since_epoch())
                .count();
        std::ostringstream fallback_id;
        fallback_id << "core-" << unix_ms << '-' << pid;
        core_instance_id = fallback_id.str();
    }

    CoreIdentity identity;
    identity.core_instance_id = std::move(core_instance_id);
    identity.pid = pid;
    identity.core_path = ecnuvpn::platform::get_executable_path();
    if (identity.core_path.empty()) {
        identity.core_path = "unknown";
    }
    identity.started_at = render_started_at(now);
    return identity;
}

nlohmann::json core_hello_payload(const CoreIdentity& identity) {
    return nlohmann::json{
        {"ipc_protocol_version", ipc_protocol_version()},
        {"contract_version",
         std::string(exv::contracts::generated::CONTRACT_VERSION)},
        {"app_version", ECNUVPN_VERSION},
        {"core_instance_id", identity.core_instance_id},
        {"pid", identity.pid},
        {"core_path", identity.core_path},
        {"started_at", identity.started_at},
    };
}

bool accepts_contract_version(std::string_view requested) {
    return requested.empty() ||
           requested == exv::contracts::generated::CONTRACT_VERSION;
}

} // namespace exv::core::lifecycle
