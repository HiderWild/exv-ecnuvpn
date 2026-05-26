#include "platform/common/proxy_tun_detector.hpp"

#include <iostream>
#include <string>

namespace {

int failures = 0;

void expect(bool condition, const std::string &name) {
  if (condition)
    return;
  ++failures;
  std::cerr << "FAILED: " << name << "\n";
}

} // namespace

int main() {
  using ecnuvpn::platform::is_proxy_tun_candidate;
  using ecnuvpn::platform::make_proxy_tun_adapter;

  expect(is_proxy_tun_candidate("Mihomo", "Mihomo TUN",
                                "default route; fake-ip 198.18.0.0/15", ""),
         "mihomo default route is proxy tun");
  expect(is_proxy_tun_candidate("sing-box", "sing-box tun",
                                "split-default route 0.0.0.0/1", ""),
         "sing-box split default is proxy tun");
  expect(is_proxy_tun_candidate("utun5", "utun interface",
                                "fake-ip route 198.18.0.0/15", ""),
         "generic virtual interface with fake-ip route is proxy tun");

  expect(!is_proxy_tun_candidate("VMware Network Adapter VMnet8", "VMware",
                                 "default route", ""),
         "vmware is excluded");
  expect(!is_proxy_tun_candidate("DockerNAT", "Hyper-V Virtual Ethernet",
                                 "default route", ""),
         "docker hyper-v is excluded");
  expect(!is_proxy_tun_candidate("Tailscale", "WireGuard Tunnel",
                                 "default route", ""),
         "tailscale wireguard is excluded");
  expect(!is_proxy_tun_candidate("Wintun", "Wintun Userspace Tunnel",
                                 "", ""),
         "wintun without route evidence is not enough");
  expect(!is_proxy_tun_candidate("ECNUVPN-1234", "OpenConnect Tunnel",
                                 "default route", "ECNUVPN-1234"),
         "exv adapter is excluded");

  auto adapter = make_proxy_tun_adapter("Mihomo", "Mihomo TUN", "42",
                                        "default route");
  expect(adapter.kind == "proxy_tun", "adapter kind set");
  expect(adapter.role == "internet_proxy", "adapter role set");
  expect(adapter.if_index == "42", "adapter if_index set");
  expect(adapter.route_reason == "default route", "adapter route reason set");

  return failures == 0 ? 0 : 1;
}
