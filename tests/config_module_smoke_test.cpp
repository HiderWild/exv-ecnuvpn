import exv.config.contract;

#include "contracts/generated/system_contract.hpp"

#include <iostream>
#include <string_view>

namespace {

bool expect(bool condition, std::string_view message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

bool check_action(std::string_view action) {
  return expect(exv::contracts::generated::is_config_action(action),
                "generated contract must include config action") &&
         expect(exv::config::contract::is_action(action.data()),
                "config module must include generated config action");
}

bool check_alias(std::string_view alias, std::string_view target) {
  return expect(exv::contracts::generated::is_config_alias(alias),
                "generated contract must include config alias") &&
         expect(exv::config::contract::is_alias(alias.data()),
                "config module must include generated config alias") &&
         expect(std::string_view{
                    exv::config::contract::canonical_action(alias.data())} ==
                    target,
                "config module alias target must match generated contract");
}

bool generated_alias_matches(std::string_view alias, std::string_view target) {
  for (const auto &candidate : exv::contracts::generated::CONFIG_ALIASES) {
    if (candidate.alias == alias) {
      return expect(candidate.target == target,
                    "generated config alias target must match expectation");
    }
  }
  std::cerr << "EXPECT FAILED: missing generated config alias " << alias
            << '\n';
  return false;
}

} // namespace

int main() {
  bool ok = true;

  ok = expect(exv::config::contract::action_count() ==
                  exv::contracts::generated::CONFIG_ACTIONS.size(),
              "config module action count must match generated contract") &&
       ok;
  ok = expect(exv::config::contract::alias_count() ==
                  exv::contracts::generated::CONFIG_ALIASES.size(),
              "config module alias count must match generated contract") &&
       ok;

  for (const auto action : exv::contracts::generated::CONFIG_ACTIONS) {
    ok = check_action(action) && ok;
  }

  for (const auto &alias : exv::contracts::generated::CONFIG_ALIASES) {
    ok = check_alias(alias.alias, alias.target) && ok;
  }

  ok = expect(exv::config::contract::is_action("config.profile.save"),
              "config module must include generated config.profile.save action") &&
       ok;
  ok = generated_alias_matches("config.get", "config.getSettings") && ok;
  ok = expect(std::string_view{
                  exv::config::contract::canonical_action("config.get")} ==
                  "config.getSettings",
              "config module config.get alias must match generated target") &&
       ok;

  ok = expect(!exv::config::contract::is_action("vpn.connect"),
              "config module must reject non-config actions") &&
       ok;
  ok = expect(exv::config::contract::canonical_action("config.unknown") ==
                  nullptr,
              "config module must return empty target for unknown aliases") &&
       ok;

  if (!ok) {
    std::cerr << "config_module_smoke_test: FAILED\n";
    return 1;
  }
  std::cout << "config_module_smoke_test: all assertions passed\n";
  return 0;
}
