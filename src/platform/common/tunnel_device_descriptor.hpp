#pragma once
#include <string>
#include <cstdint>

namespace exv::platform {

struct TunnelDeviceDescriptor {
    std::string path;          // e.g. \\.\Wintun\... or /dev/tunN
    std::string adapter_name;  // Human-readable name
    int fd = -1;               // Unix fd (Linux/macOS)
    void* handle = nullptr;    // Windows HANDLE
    int mtu = 1400;
    bool is_open = false;
    std::string error_code;
    std::string error_message;
};

} // namespace exv::platform
