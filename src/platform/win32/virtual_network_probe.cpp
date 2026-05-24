#include "platform/common/virtual_network_probe.hpp"

#include "utils.hpp"

#include <algorithm>
#include <cctype>
#include <set>
#include <sstream>
#include <vector>

namespace ecnuvpn {
namespace platform {
namespace {

std::string lower_ascii(std::string value) {
  std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
    return static_cast<char>(std::tolower(c));
  });
  return value;
}

bool contains_any_virtual_token(const std::string &value) {
  std::string lower = lower_ascii(value);
  const char *tokens[] = {"clash",     "mihomo",  "meta",      "sing-box",
                          "singbox",   "tun2socks", "wireguard", "tailscale",
                          "zerotier",  "openvpn", "wintun",    "tap",
                          "utun",      "vpn"};
  for (const char *token : tokens) {
    if (lower.find(token) != std::string::npos)
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
  return lower_name.find("ecnuvpn") != std::string::npos ||
         lower_detail.find("openconnect tunnel") != std::string::npos;
}

std::vector<std::string> split_pipe_fields(const std::string &line) {
  std::vector<std::string> fields;
  std::string field;
  std::istringstream iss(line);
  while (std::getline(iss, field, '|')) {
    fields.push_back(utils::trim(field));
  }
  return fields;
}

void add_adapter(std::vector<virtual_network::AdapterInfo> *adapters,
                 std::set<std::string> *seen, const std::string &name,
                 const std::string &detail, const std::string &exv_interface) {
  if (!adapters || !seen)
    return;

  std::string clean_name = utils::trim(name);
  std::string clean_detail = utils::trim(detail);
  if (clean_name.empty() ||
      is_exv_adapter(clean_name, clean_detail, exv_interface)) {
    return;
  }

  std::string key = lower_ascii(clean_name);
  if (seen->find(key) != seen->end())
    return;
  seen->insert(key);
  adapters->push_back(virtual_network::AdapterInfo{clean_name, clean_detail});
}

} // namespace

std::vector<virtual_network::AdapterInfo>
detect_virtual_network_adapters(const std::string &exv_interface) {
  std::vector<virtual_network::AdapterInfo> adapters;
  std::set<std::string> seen;
  std::string command =
      "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "
      "\"$patterns='clash|mihomo|meta|sing-box|singbox|tun2socks|wireguard|tailscale|zerotier|openvpn|vpn|wintun|tap|tun'; "
      "Get-NetAdapter -ErrorAction SilentlyContinue | Where-Object { "
      "$_.Status -eq 'Up' -and $_.Name -notlike 'ECNUVPN*' -and "
      "$_.InterfaceDescription -notlike 'OpenConnect Tunnel*' -and "
      "(($_.Name -match $patterns) -or ($_.InterfaceDescription -match $patterns)) "
      "} | ForEach-Object { 'adapter|' + $_.Name + '|' + $_.InterfaceDescription + '|' + $_.ifIndex }; "
      "Get-NetRoute -AddressFamily IPv4 -ErrorAction SilentlyContinue | Where-Object { "
      "$_.InterfaceAlias -notlike 'ECNUVPN*' -and "
      "(($_.DestinationPrefix -like '198.18.*') -or ($_.DestinationPrefix -like '198.19.*') -or "
      "($_.NextHop -like '198.18.*') -or ($_.NextHop -like '198.19.*')) "
      "} | ForEach-Object { 'route|' + $_.InterfaceAlias + '|' + $_.DestinationPrefix + ' via ' + $_.NextHop + '|' + $_.InterfaceIndex }\"";

  for (const auto &line : utils::split_lines(utils::run_command_output(command))) {
    std::vector<std::string> fields = split_pipe_fields(line);
    if (fields.size() < 3)
      continue;

    if (fields[0] == "adapter") {
      add_adapter(&adapters, &seen, fields[1], fields[2], exv_interface);
    } else if (fields[0] == "route") {
      std::string detail = fields[2];
      if (detail.find("198.18.") != std::string::npos ||
          detail.find("198.19.") != std::string::npos ||
          contains_any_virtual_token(fields[1])) {
        add_adapter(&adapters, &seen, fields[1], detail, exv_interface);
      }
    }
  }

  return adapters;
}

} // namespace platform
} // namespace ecnuvpn