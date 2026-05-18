#include "virtual_network.hpp"

#include "utils.hpp"

#include <algorithm>
#include <cctype>
#include <set>
#include <sstream>

namespace ecnuvpn {
namespace virtual_network {
namespace {

std::string lower_ascii(std::string value) {
  std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
    return static_cast<char>(std::tolower(c));
  });
  return value;
}

bool contains_any_virtual_token(const std::string &value) {
  std::string lower = lower_ascii(value);
  const char *tokens[] = {"clash",   "mihomo",  "meta",    "sing-box",
                          "singbox", "tun2socks", "wireguard", "tailscale",
                          "zerotier", "openvpn", "wintun",  "tap",
                          "utun",    "vpn"};
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

void add_adapter(std::vector<AdapterInfo> *adapters, std::set<std::string> *seen,
                 const std::string &name, const std::string &detail,
                 const std::string &exv_interface) {
  if (!adapters || !seen)
    return;
  std::string clean_name = utils::trim(name);
  std::string clean_detail = utils::trim(detail);
  if (clean_name.empty() || is_exv_adapter(clean_name, clean_detail, exv_interface))
    return;

  std::string key = lower_ascii(clean_name);
  if (seen->find(key) != seen->end())
    return;
  seen->insert(key);
  adapters->push_back(AdapterInfo{clean_name, clean_detail});
}

std::string join_adapter_names(const std::vector<AdapterInfo> &adapters) {
  std::string joined;
  for (size_t i = 0; i < adapters.size(); ++i) {
    if (i > 0)
      joined += ", ";
    joined += adapters[i].name;
  }
  return joined;
}

#ifdef _WIN32
Detection detect_windows(const std::string &exv_interface) {
  Detection detection;
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
      add_adapter(&detection.adapters, &seen, fields[1], fields[2], exv_interface);
    } else if (fields[0] == "route") {
      std::string detail = fields[2];
      if (detail.find("198.18.") != std::string::npos ||
          detail.find("198.19.") != std::string::npos ||
          contains_any_virtual_token(fields[1])) {
        add_adapter(&detection.adapters, &seen, fields[1], detail, exv_interface);
      }
    }
  }
  return detection;
}
#else
Detection detect_posix(const std::string &exv_interface) {
  Detection detection;
  std::set<std::string> seen;
#ifdef __APPLE__
  std::string command =
      "netstat -rn -f inet 2>/dev/null | "
      "awk '/198\\.18|198\\.19/ {print \"route|\" $NF \"|\" $1 \" via \" $2}'";
#else
  std::string command =
      "ip route show 2>/dev/null | "
      "awk '/198\\.18|198\\.19|clash|mihomo|sing-box|tun2socks|wireguard|tailscale|zerotier/ "
      "{iface=\"\"; for (i=1;i<=NF;i++) if ($i==\"dev\" && i<NF) iface=$(i+1); "
      "if (iface != \"\") print \"route|\" iface \"|\" $0}'";
#endif

  for (const auto &line : utils::split_lines(utils::run_command_output(command))) {
    std::vector<std::string> fields = split_pipe_fields(line);
    if (fields.size() < 3)
      continue;
    add_adapter(&detection.adapters, &seen, fields[1], fields[2], exv_interface);
  }
  return detection;
}
#endif

void finalize_detection(Detection *detection) {
  if (!detection)
    return;
  detection->detected = !detection->adapters.empty();
  if (detection->detected) {
    detection->message =
        "发现其他虚拟网卡（" + join_adapter_names(detection->adapters) +
        "），正在把 EXV 串联到它们前面提前路由校园流量；其他流量继续按系统默认出口处理。";
  }
}

} // namespace

Detection detect_upstream_virtual_network(const std::string &exv_interface) {
#ifdef _WIN32
  Detection detection = detect_windows(exv_interface);
#else
  Detection detection = detect_posix(exv_interface);
#endif
  finalize_detection(&detection);
  return detection;
}

nlohmann::json to_json(const Detection &detection) {
  nlohmann::json adapters = nlohmann::json::array();
  for (const auto &adapter : detection.adapters) {
    adapters.push_back({{"name", adapter.name}, {"detail", adapter.detail}});
  }
  return nlohmann::json{{"detected", detection.detected},
                        {"adapters", adapters},
                        {"message", detection.message},
                        {"route_policy", detection.detected
                                             ? "exv-campus-routes-first"
                                             : "normal"}};
}

void add_status_fields(nlohmann::json &status, const std::string &exv_interface) {
  Detection detection = detect_upstream_virtual_network(exv_interface);
  nlohmann::json j = to_json(detection);
  status["upstream_virtual_detected"] = j["detected"];
  status["upstream_virtual_adapters"] = j["adapters"];
  status["upstream_virtual_message"] = j["message"];
  status["route_policy"] = j["route_policy"];
}

void print_notice(const Detection &detection) {
  if (!detection.detected)
    return;
  utils::print_warning(detection.message);
}

} // namespace virtual_network
} // namespace ecnuvpn
