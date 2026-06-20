// Generated from distribution/ecnu.json by scripts/generate_distribution_config.py.
// Do not edit by hand.
#pragma once

#include <array>
#include <string_view>

namespace exv::distribution {

inline constexpr std::string_view kId = "ecnu";
inline constexpr std::string_view kAppName = "EXV";
inline constexpr std::string_view kBrandSubtitle = "for ECNU";
inline constexpr std::string_view kAuthor = "HiderWild";
inline constexpr std::string_view kRepositoryLabel = "HiderWild/easy-ecnu-vpn";
inline constexpr std::string_view kRepositoryUrl = "https://github.com/HiderWild/easy-ecnu-vpn";
inline constexpr std::string_view kDefaultVpnServer = "vpn-cn.ecnu.edu.cn";

inline constexpr std::array<std::string_view, 3> kVpnServers = {
    "vpn-cn.ecnu.edu.cn",
    "vpn-ct.ecnu.edu.cn",
    "vpn-lt.ecnu.edu.cn"
};

inline constexpr std::array<std::string_view, 9> kDefaultRoutes = {
    "49.52.4.0/25",
    "59.78.176.0/20",
    "59.78.199.0/21",
    "58.198.176.128/25",
    "219.228.60.69",
    "59.78.189.128/25",
    "219.228.63.0/21",
    "202.120.80.0/20",
    "222.66.117.0/24"
};

inline constexpr std::string_view kDefaultWindowsUserAgent = "AnyConnect Win_x86_64 4.10.05095";
inline constexpr std::string_view kDefaultMacosUserAgent = "AnyConnect Darwin_x86_64 4.10.05095";
inline constexpr std::string_view kDefaultLinuxUserAgent = "AnyConnect Linux_x86_64 4.10.05095";

#if defined(EXV_PLATFORM_WINDOWS)
inline constexpr std::string_view kDefaultUserAgent = kDefaultWindowsUserAgent;
#elif defined(EXV_PLATFORM_DARWIN)
inline constexpr std::string_view kDefaultUserAgent = kDefaultMacosUserAgent;
#else
inline constexpr std::string_view kDefaultUserAgent = kDefaultLinuxUserAgent;
#endif

} // namespace exv::distribution
