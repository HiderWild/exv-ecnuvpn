#include "platform/common/file_system.hpp"
#include "platform/common/interface_stats.hpp"
#include "platform/common/process_utils.hpp"
#include "platform/common/runtime_discovery.hpp"
#include "platform/common/runtime_paths.hpp"
#include "core/app_api/app_api.hpp"

#include <chrono>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

void dump_json(const char *label, const nlohmann::json &value) {
  std::cerr << label << ": " << value.dump(2) << std::endl;
}

struct RuntimePathGuard {
  ~RuntimePathGuard() { exv::platform::clear_runtime_path_override(); }
};

bool logs_clear_truncates_runtime_log_file() {
  RuntimePathGuard guard;

  const auto temp_root =
      std::filesystem::temp_directory_path() /
      ("exv-app-api-logs-test-" +
       std::to_string(
           std::chrono::steady_clock::now().time_since_epoch().count()));
  std::filesystem::create_directories(temp_root);

  const auto log_path = temp_root / "exv.log";
  {
    std::ofstream out(log_path.string(), std::ios::out | std::ios::trunc);
    out << "[INFO] marker-line-1\n";
    out << "[ERROR] marker-line-2\n";
  }

  const nlohmann::json payload{
      {"home", temp_root.string()},
      {"config_dir", temp_root.string()},
      {"lines", 100},
  };

  const auto before = exv::app_api::handle_action("logs.list", payload);
  const auto cleared = exv::app_api::handle_action("logs.clear", payload);
  const auto after = exv::app_api::handle_action("logs.list", payload);

  bool ok = true;
  ok = expect(before.is_array() && !before.empty(),
              "logs.list should expose the existing runtime log lines") &&
       ok;
  bool saw_marker = false;
  if (before.is_array()) {
    for (const auto &entry : before) {
      if (entry.is_object() &&
          entry.value("message", std::string()).find("marker-line-1") !=
              std::string::npos) {
        saw_marker = true;
        break;
      }
    }
  }
  if (!saw_marker)
    dump_json("before", before);
  ok = expect(saw_marker,
              "logs.list should read the overridden runtime log file") &&
       ok;
  if (!(cleared.is_object() && cleared.value("ok", false)))
    dump_json("cleared", cleared);
  ok = expect(cleared.is_object() && cleared.value("ok", false),
              "logs.clear should succeed") &&
       ok;

  std::ifstream in(log_path.string(), std::ios::in);
  std::string contents((std::istreambuf_iterator<char>(in)),
                       std::istreambuf_iterator<char>());
  in.close();
  ok = expect(contents.empty(),
              "logs.clear should truncate the runtime log file") &&
       ok;
  if (!(after.is_array() && after.empty()))
    dump_json("after", after);
  ok = expect(after.is_array() && after.empty(),
              "logs.list should be empty after logs.clear") &&
       ok;
  return ok;
}

} // namespace

int main() { return logs_clear_truncates_runtime_log_file() ? 0 : 1; }
