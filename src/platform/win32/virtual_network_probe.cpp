#include "utils/strings.hpp"
#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "platform/common/virtual_network_probe.hpp"

#include "platform/common/proxy_tun_detector.hpp"

#include <algorithm>
#include <cctype>
#include <map>
#include <set>
#include <sstream>
#include <string>
#include <vector>

namespace exv {
namespace platform {
namespace {

struct Candidate {
  std::string name;
  std::string detail;
  std::string if_index;
  std::set<std::string> reasons;
};

std::string lower_ascii(std::string value) {
  std::transform(value.begin(), value.end(), value.begin(), [](unsigned char c) {
    return static_cast<char>(std::tolower(c));
  });
  return value;
}

std::vector<std::string> split_pipe_fields(const std::string &line) {
  std::vector<std::string> fields;
  std::string field;
  std::istringstream iss(line);
  while (std::getline(iss, field, '|')) {
    fields.push_back(exv::utils::trim(field));
  }
  return fields;
}

std::string route_reason_for_prefix(const std::string &prefix,
                                    const std::string &next_hop,
                                    const std::string &metric) {
  std::string lower_prefix = lower_ascii(prefix);
  std::string lower_next_hop = lower_ascii(next_hop);
  std::string reason;
  if (lower_prefix == "0.0.0.0/0") {
    reason = "default route";
  } else if (lower_prefix == "0.0.0.0/1" ||
             lower_prefix == "128.0.0.0/1") {
    reason = "split-default route " + prefix;
  } else if (lower_prefix.rfind("198.18.", 0) == 0 ||
             lower_prefix.rfind("198.19.", 0) == 0 ||
             lower_next_hop.rfind("198.18.", 0) == 0 ||
             lower_next_hop.rfind("198.19.", 0) == 0) {
    reason = "fake-ip route " + prefix;
  }
  if (!reason.empty() && !metric.empty())
    reason += " metric " + metric;
  return reason;
}

std::string join_reasons(const std::set<std::string> &reasons) {
  std::string joined;
  for (const auto &reason : reasons) {
    if (!joined.empty())
      joined += "; ";
    joined += reason;
  }
  return joined;
}

Candidate &candidate_for(std::map<std::string, Candidate> *candidates,
                         const std::string &if_index,
                         const std::string &alias = "") {
  Candidate &candidate = (*candidates)[if_index.empty() ? alias : if_index];
  if (!if_index.empty())
    candidate.if_index = if_index;
  if (!alias.empty() && candidate.name.empty())
    candidate.name = alias;
  return candidate;
}

} // namespace

std::vector<virtual_network::AdapterInfo>
detect_virtual_network_adapters(const std::string &exv_interface) {
  std::map<std::string, Candidate> candidates;

  std::string command =
      "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "
      "\"Get-NetAdapter -ErrorAction SilentlyContinue | "
      "ForEach-Object { 'adapter|' + $_.ifIndex + '|' + $_.Name + '|' + $_.InterfaceDescription + '|' + $_.Status }; "
      "Get-NetRoute -AddressFamily IPv4 -ErrorAction SilentlyContinue | "
      "Where-Object { $_.DestinationPrefix -in @('0.0.0.0/0','0.0.0.0/1','128.0.0.0/1') -or "
      "$_.DestinationPrefix -like '198.18.*' -or $_.DestinationPrefix -like '198.19.*' -or "
      "$_.NextHop -like '198.18.*' -or $_.NextHop -like '198.19.*' } | "
      "Sort-Object RouteMetric,InterfaceMetric | "
      "ForEach-Object { 'route|' + $_.InterfaceIndex + '|' + $_.InterfaceAlias + '|' + $_.DestinationPrefix + '|' + $_.NextHop + '|' + $_.RouteMetric }\"";

  for (const auto &line : exv::utils::split_lines(platform::run_command_output(command))) {
    std::vector<std::string> fields = split_pipe_fields(line);
    if (fields.empty())
      continue;

    if (fields[0] == "adapter" && fields.size() >= 5) {
      Candidate &candidate = candidate_for(&candidates, fields[1], fields[2]);
      candidate.detail = fields[3];
      continue;
    }

    if (fields[0] == "route" && fields.size() >= 6) {
      std::string reason = route_reason_for_prefix(fields[3], fields[4], fields[5]);
      if (reason.empty())
        continue;
      Candidate &candidate = candidate_for(&candidates, fields[1], fields[2]);
      candidate.reasons.insert(reason);
    }
  }

  std::vector<virtual_network::AdapterInfo> adapters;
  std::set<std::string> seen;
  for (const auto &[_, candidate] : candidates) {
    std::string route_reason = join_reasons(candidate.reasons);
    if (!is_proxy_tun_candidate(candidate.name, candidate.detail, route_reason,
                                exv_interface)) {
      continue;
    }
    std::string key = lower_ascii(candidate.name);
    if (seen.insert(key).second) {
      adapters.push_back(make_proxy_tun_adapter(candidate.name, candidate.detail,
                                                candidate.if_index,
                                                route_reason));
    }
  }
  return adapters;
}

} // namespace platform
} // namespace exv
