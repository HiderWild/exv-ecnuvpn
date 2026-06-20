#include "core/rpc/log_actions.hpp"
#include "platform/common/runtime_paths.hpp"

#include <nlohmann/json.hpp>

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>

namespace {

using json = nlohmann::json;

bool expect(bool condition, const char *message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

struct RuntimePathGuard {
  RuntimePathGuard() {
    temp_root =
        std::filesystem::temp_directory_path() /
        ("ecnuvpn-log-actions-test-" +
         std::to_string(
             std::chrono::steady_clock::now().time_since_epoch().count()));
    std::filesystem::create_directories(temp_root);
    ecnuvpn::platform::set_runtime_path_override(temp_root.string(),
                                                 temp_root.string());
  }

  ~RuntimePathGuard() {
    ecnuvpn::platform::clear_runtime_path_override();
    std::error_code ec;
    std::filesystem::remove_all(temp_root, ec);
  }

  std::filesystem::path temp_root;
};

json invoke_logs_list(const json &payload) {
  exv::core_api::LogActions logs;
  exv::core_api::RpcRequest req;
  req.action = "logs.list";
  req.payload_json = payload.dump();

  const auto resp = logs.list(req);
  if (!resp.success) {
    return {{"ok", false},
            {"code", resp.error_code},
            {"message", resp.error_message}};
  }
  return json::parse(resp.payload_json);
}

bool logs_list_honors_incremental_limit_and_sequence() {
  RuntimePathGuard guard;

  const auto log_path = guard.temp_root / "ecnuvpn.log";
  {
    std::ofstream out(log_path.string(), std::ios::out | std::ios::trunc);
    for (int i = 1; i <= 500; ++i) {
      out << "[INFO] marker-line-" << i << '\n';
    }
  }

  const auto first = invoke_logs_list({{"limit", 50}});

  bool ok = true;
  ok = expect(first.is_array(), "logs.list should keep array response shape") &&
       ok;
  ok = expect(first.size() == 50, "logs.list should cap limit-sized reads") &&
       ok;
  if (first.is_array() && !first.empty()) {
    ok = expect(first.front().value("seq", 0) == 451,
                "initial limited read should return the tail sequence") &&
         ok;
    ok = expect(first.back().value("seq", 0) == 500,
                "tail sequence should include the last physical log line") &&
         ok;
  }

  {
    std::ofstream out(log_path.string(), std::ios::out | std::ios::app);
    out << "[INFO] marker-line-501\n";
    out << "[INFO] marker-line-502\n";
  }

  const auto after = invoke_logs_list({{"after_seq", 500}, {"limit", 50}});
  ok = expect(after.is_array(), "incremental logs.list should return an array") &&
       ok;
  ok = expect(after.size() == 2,
              "incremental logs.list should only return entries after seq") &&
       ok;
  if (after.is_array() && after.size() == 2) {
    ok = expect(after[0].value("seq", 0) == 501,
                "incremental read should preserve physical line sequence") &&
         ok;
    ok = expect(after[1].value("seq", 0) == 502,
                "incremental read should advance sequence") &&
         ok;
  }

  return ok;
}

bool logs_list_keeps_single_response_under_pipe_budget() {
  RuntimePathGuard guard;

  const auto log_path = guard.temp_root / "ecnuvpn.log";
  const std::string long_message(300, 'x');
  {
    std::ofstream out(log_path.string(), std::ios::out | std::ios::trunc);
    for (int i = 1; i <= 100; ++i) {
      out << "[INFO] long-line-" << i << ' ' << long_message << '\n';
    }
  }

  const auto chunk = invoke_logs_list({{"limit", 200}});

  bool ok = true;
  ok = expect(chunk.is_array(), "budgeted logs.list should return an array") &&
       ok;
  ok = expect(!chunk.empty(), "budgeted logs.list should still return logs") &&
       ok;
  ok = expect(chunk.dump().size() < 7000,
              "logs.list payload should stay below the pipe read budget") &&
       ok;
  if (chunk.is_array() && !chunk.empty()) {
    ok = expect(chunk.back().value("seq", 0) == 100,
                "budgeted tail should keep the newest log line") &&
         ok;
  }
  return ok;
}

} // namespace

int main() {
  if (!logs_list_honors_incremental_limit_and_sequence()) {
    return 1;
  }
  if (!logs_list_keeps_single_response_under_pipe_budget()) {
    return 1;
  }
  std::cout << "log_actions_test: all assertions passed\n";
  return 0;
}
