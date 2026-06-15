import exv.helper.protocol;

#include "contracts/generated/system_contract.hpp"

#include <cstdint>
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

bool check_op(std::string_view name, exv::helper::protocol::HelperOp op,
              bool requires_session) {
  for (const auto &contract :
       exv::contracts::generated::HELPER_OP_CONTRACTS) {
    if (contract.name != name) {
      continue;
    }
    return expect(contract.code ==
                      exv::helper::protocol::helper_op_code(op),
                  "module helper op code must match generated contract") &&
           expect(contract.requires_session == requires_session,
                  "module helper op session flag must match contract") &&
           expect(exv::helper::protocol::helper_op_requires_session(op) ==
                      requires_session,
                  "module helper op session helper must match contract");
  }

  std::cerr << "EXPECT FAILED: missing generated helper op " << name << '\n';
  return false;
}

} // namespace

int main() {
  bool ok = true;
  using exv::helper::protocol::HelperMode;
  using exv::helper::protocol::HelperOp;

  ok = expect(exv::contracts::generated::HELPER_OP_CONTRACTS.size() ==
                  exv::helper::protocol::helper_op_count(),
              "module helper op count must match generated contract") &&
       ok;

  ok = check_op("Hello", HelperOp::Hello, false) && ok;
  ok = check_op("StartSession", HelperOp::StartSession, false) && ok;
  ok = check_op("PrepareTunnelDevice", HelperOp::PrepareTunnelDevice, true) &&
       ok;
  ok = check_op("ApplyTunnelConfig", HelperOp::ApplyTunnelConfig, true) && ok;
  ok = check_op("Heartbeat", HelperOp::Heartbeat, true) && ok;
  ok = check_op("Cleanup", HelperOp::Cleanup, true) && ok;
  ok = check_op("GetSnapshot", HelperOp::GetSnapshot, false) && ok;
  ok = check_op("Shutdown", HelperOp::Shutdown, true) && ok;
  ok = expect(exv::helper::protocol::helper_mode_code(
                  HelperMode::Transient) == 1,
              "module transient helper mode must match wire value") &&
       ok;
  ok = expect(exv::helper::protocol::helper_mode_code(
                  HelperMode::Resident) == 2,
              "module resident helper mode must match wire value") &&
       ok;

  if (!ok) {
    std::cerr << "helper_module_smoke_test: FAILED\n";
    return 1;
  }
  std::cout << "helper_module_smoke_test: all tests passed\n";
  return 0;
}
