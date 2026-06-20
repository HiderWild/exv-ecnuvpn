#include "platform/common/proxy_tun_detector.hpp"

#include <algorithm>
#include <cctype>

namespace exv {
namespace platform {
namespace {

std::string lower_ascii(std::string value) {
  std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
    return static_cast<char>(std::tolower(c));
  });
  return value;
}

bool contains_any(const std::string &value, const char *const *tokens,
                  std::size_t count) {
  for (std::size_t i = 0; i < count; ++i) {
    if (value.find(tokens[i]) != std::string::npos)
      return true;
  }
  return false;
}

bool is_exv_adapter(const std::string &name, const std::string &detail,
                    const std::string &exv_interface) {
  std::string lower_name = lower_ascii(name);
  std::string lower_detail = lower_ascii(detail);
  std::string lower_exv = lower_ascii(exv_interface);
  if (!lower_exv.empty() && lower_name == lower_exv)
    return true;
  return lower_name.find("exv") != std::string::npos ||
         lower_detail.find("openconnect tunnel") != std::string::npos;
}

bool is_blacklisted_adapter(const std::string &combined) {
  const char *tokens[] = {"vmware",   "virtualbox", "hyper-v",  "hyperv",
                          "vethernet", "docker",    "wsl",      "bluetooth",
                          "loopback", "tailscale",  "wireguard", "openvpn",
                          "zerotier"};
  return contains_any(combined, tokens, sizeof(tokens) / sizeof(tokens[0]));
}

bool has_proxy_token(const std::string &combined) {
  const char *tokens[] = {"clash",       "mihomo",   "sing-box", "singbox",
                          "tun2socks",   "nekoray",  "nekobox",  "hiddify",
                          "surge",       "stash",    "loon",     "quantumult",
                          "shadowrocket"};
  return contains_any(combined, tokens, sizeof(tokens) / sizeof(tokens[0]));
}

bool is_virtualish_adapter(const std::string &combined) {
  const char *tokens[] = {"tun", "tap", "utun", "wintun", "loopback"};
  return contains_any(combined, tokens, sizeof(tokens) / sizeof(tokens[0]));
}

bool has_route_impact(const std::string &reason) {
  return reason.find("default") != std::string::npos ||
         reason.find("split-default") != std::string::npos ||
         reason.find("fake-ip") != std::string::npos;
}

bool has_fake_ip_evidence(const std::string &reason) {
  return reason.find("fake-ip") != std::string::npos ||
         reason.find("198.18") != std::string::npos ||
         reason.find("198.19") != std::string::npos;
}

} // namespace

bool is_proxy_tun_candidate(const std::string &name,
                            const std::string &detail,
                            const std::string &route_reason,
                            const std::string &exv_interface) {
  if (name.empty() || route_reason.empty())
    return false;

  if (is_exv_adapter(name, detail, exv_interface))
    return false;

  std::string combined = lower_ascii(name + " " + detail);
  std::string reason = lower_ascii(route_reason);
  if (is_blacklisted_adapter(combined))
    return false;
  if (!has_route_impact(reason))
    return false;

  if (has_proxy_token(combined))
    return true;

  return has_fake_ip_evidence(reason) && is_virtualish_adapter(combined);
}

virtual_network::AdapterInfo make_proxy_tun_adapter(
    const std::string &name, const std::string &detail,
    const std::string &if_index, const std::string &route_reason) {
  return virtual_network::AdapterInfo{name, detail, "proxy_tun",
                                      "internet_proxy", if_index,
                                      route_reason};
}

} // namespace platform
} // namespace exv
