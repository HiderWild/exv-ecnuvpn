import exv.core.tunnel.contract;

#include "contracts/generated/system_contract.hpp"

#include <iostream>
#include <span>
#include <string>
#include <string_view>

namespace {

bool expect(bool condition, std::string_view message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

bool check_phase(
    const exv::contracts::generated::TunnelPhaseContract &generated) {
  const std::string name{generated.name};
  const auto *module_phase =
      exv::core::tunnel::contract::phase_contract(name.c_str());
  if (module_phase == nullptr) {
    std::cerr << "EXPECT FAILED: tunnel module missing phase "
              << generated.name << '\n';
    return false;
  }

  bool ok = true;
  ok = expect(std::string_view{module_phase->name} == generated.name,
              "tunnel module phase name must match generated contract") &&
       ok;
  ok = expect(std::string_view{module_phase->wire_name} == generated.wire_name,
              "tunnel module phase wire name must match generated contract") &&
       ok;
  ok = expect(module_phase->running == generated.running,
              "tunnel module phase running flag must match generated contract") &&
       ok;
  ok = expect(module_phase->connected == generated.connected,
              "tunnel module phase connected flag must match generated contract") &&
       ok;
  ok = expect(module_phase->network_ready == generated.network_ready,
              "tunnel module phase network-ready flag must match generated contract") &&
       ok;
  return ok;
}

bool check_name_array(std::span<const std::string_view> generated,
                      bool (*module_contains)(const char *),
                      std::string_view label) {
  bool ok = true;
  for (const auto value : generated) {
    const std::string name{value};
    ok = expect(module_contains(name.c_str()),
                "tunnel module must include generated " + std::string{label}) &&
         ok;
  }
  return ok;
}

} // namespace

int main() {
  bool ok = true;

  ok = expect(exv::core::tunnel::contract::phase_count() ==
                  exv::contracts::generated::TUNNEL_PHASE_CONTRACTS.size(),
              "tunnel module phase count must match generated contract") &&
       ok;
  ok = expect(exv::core::tunnel::contract::event_count() ==
                  exv::contracts::generated::TUNNEL_EVENTS.size(),
              "tunnel module event count must match generated contract") &&
       ok;
  ok = expect(exv::core::tunnel::contract::disconnect_reason_count() ==
                  exv::contracts::generated::TUNNEL_DISCONNECT_REASONS.size(),
              "tunnel module disconnect reason count must match generated contract") &&
       ok;
  ok = expect(exv::core::tunnel::contract::error_domain_count() ==
                  exv::contracts::generated::TUNNEL_ERROR_DOMAINS.size(),
              "tunnel module error domain count must match generated contract") &&
       ok;
  ok = expect(exv::core::tunnel::contract::status_field_count() ==
                  exv::contracts::generated::TUNNEL_STATUS_FIELDS.size(),
              "tunnel module status field count must match generated contract") &&
       ok;

  for (const auto &phase :
       exv::contracts::generated::TUNNEL_PHASE_CONTRACTS) {
    ok = check_phase(phase) && ok;
  }

  ok = check_name_array(exv::contracts::generated::TUNNEL_EVENTS,
                        exv::core::tunnel::contract::is_event, "event") &&
       ok;
  ok = check_name_array(exv::contracts::generated::TUNNEL_DISCONNECT_REASONS,
                        exv::core::tunnel::contract::is_disconnect_reason,
                        "disconnect reason") &&
       ok;
  ok = check_name_array(exv::contracts::generated::TUNNEL_ERROR_DOMAINS,
                        exv::core::tunnel::contract::is_error_domain,
                        "error domain") &&
       ok;
  ok = check_name_array(exv::contracts::generated::TUNNEL_STATUS_FIELDS,
                        exv::core::tunnel::contract::is_status_field,
                        "status field") &&
       ok;

  ok = expect(exv::core::tunnel::contract::phase_contract("Unknown") ==
                  nullptr,
              "tunnel module must reject unknown phases") &&
       ok;
  ok = expect(!exv::core::tunnel::contract::is_event("UnknownEvent"),
              "tunnel module must reject unknown events") &&
       ok;
  ok = expect(!exv::core::tunnel::contract::is_disconnect_reason("Unknown"),
              "tunnel module must reject unknown disconnect reasons") &&
       ok;
  ok = expect(!exv::core::tunnel::contract::is_error_domain("unknown"),
              "tunnel module must reject unknown error domains") &&
       ok;
  ok = expect(!exv::core::tunnel::contract::is_status_field("unknown"),
              "tunnel module must reject unknown status fields") &&
       ok;

  if (!ok) {
    std::cerr << "tunnel_module_smoke_test: FAILED\n";
    return 1;
  }
  std::cout << "tunnel_module_smoke_test: all assertions passed\n";
  return 0;
}
