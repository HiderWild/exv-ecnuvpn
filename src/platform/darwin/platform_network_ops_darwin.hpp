#pragma once

#include "platform/common/platform_network_ops.hpp"
#include "platform/darwin/native_route_config.hpp"
#include "platform/darwin/native_utun.hpp"

#include <functional>
#include <memory>
#include <string>
#include <vector>

namespace exv::platform {

struct NativeDarwinDnsSettings {
  std::vector<std::string> servers;
  std::string search_domain;
  std::vector<std::string> suffixes;
};

struct NativeDarwinDnsApi {
  std::function<int(const std::string &, const NativeDarwinDnsSettings &)>
      apply_dns;
  std::function<int(const std::string &)> restore_dns;
  std::function<int(const std::string &)> disable_interface;
};

NativeDarwinDnsApi default_native_darwin_dns_api();

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops();

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops(
    ecnuvpn::platform::NativeUtunApi utun_api,
    ecnuvpn::platform::NativeDarwinRouteApi route_api);

std::unique_ptr<PlatformNetworkOps> create_darwin_platform_network_ops(
    ecnuvpn::platform::NativeUtunApi utun_api,
    ecnuvpn::platform::NativeDarwinRouteApi route_api,
    NativeDarwinDnsApi dns_api);

} // namespace exv::platform
