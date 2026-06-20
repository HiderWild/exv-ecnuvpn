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

std::string join_reasons(const std::set<std::string> &reasons) {
  std::string joined;
  for (const auto &reason : reasons) {
    if (!joined.empty())
      joined += "; ";
    joined += reason;
  }
  return joined;
}

} // namespace

std::vector<virtual_network::AdapterInfo>
detect_virtual_network_adapters(const std::string &exv_interface) {
  std::map<std::string, Candidate> candidates;
  std::string command =
      "ip route show 2>/dev/null | "
      "awk '{iface=\"\"; for (i=1;i<=NF;i++) if ($i==\"dev\" && i<NF) iface=$(i+1); "
      "reason=\"\"; "
      "if ($1==\"default\") reason=\"default route\"; "
      "else if ($1==\"0.0.0.0/1\" || $1==\"128.0.0.0/1\") reason=\"split-default route \" $1; "
      "else if ($1 ~ /^198\\.(18|19)\\./ || $0 ~ /198\\.(18|19)\\./) reason=\"fake-ip route \" $1; "
      "if (iface != \"\" && reason != \"\") print \"route|\" iface \"|\" reason \"|\" $0}'";

  for (const auto &line : exv::utils::split_lines(platform::run_command_output(command))) {
    std::vector<std::string> fields = split_pipe_fields(line);
    if (fields.size() < 3 || fields[0] != "route" || fields[1].empty())
      continue;
    Candidate &candidate = candidates[fields[1]];
    candidate.name = fields[1];
    candidate.reasons.insert(fields[2]);
  }

  std::vector<virtual_network::AdapterInfo> adapters;
  std::set<std::string> seen;
  for (const auto &[_, candidate] : candidates) {
    std::string route_reason = join_reasons(candidate.reasons);
    if (!is_proxy_tun_candidate(candidate.name, candidate.name, route_reason,
                                exv_interface)) {
      continue;
    }
    std::string key = lower_ascii(candidate.name);
    if (seen.insert(key).second) {
      adapters.push_back(make_proxy_tun_adapter(candidate.name, candidate.name,
                                                "", route_reason));
    }
  }
  return adapters;
}

} // namespace platform
} // namespace exv
