#include "platform/common/virtual_network_probe.hpp"

#include "platform/common/proxy_tun_detector.hpp"
#include "utils.hpp"

#include <algorithm>
#include <cctype>
#include <map>
#include <set>
#include <sstream>
#include <string>
#include <vector>

namespace ecnuvpn {
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
    fields.push_back(utils::trim(field));
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
      "(route -n get default 2>/dev/null | "
      "awk '/interface:/{print \"route|\" $2 \"|default route\"}'; "
      "netstat -rn -f inet 2>/dev/null | "
      "awk '($1==\"0/1\" || $1==\"0.0.0.0/1\" || $1==\"128/1\" || $1==\"128.0.0.0/1\") "
      "{print \"route|\" $NF \"|split-default route \" $1 \" via \" $2} "
      "/198\\.18|198\\.19/ {print \"route|\" $NF \"|fake-ip route \" $1 \" via \" $2}'"
      ")";

  for (const auto &line : utils::split_lines(utils::run_command_output(command))) {
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
} // namespace ecnuvpn
