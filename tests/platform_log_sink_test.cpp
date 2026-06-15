#include "platform/common/logging/file_log_sink.hpp"
#include "platform/common/logging/stdout_log_sink.hpp"

#include <nlohmann/json.hpp>

#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>

namespace {

exv::observability::LogEvent make_event(exv::observability::LogLevel level,
                                        std::string message) {
  exv::observability::LogEvent event;
  event.level = level;
  event.component = "platform-test";
  event.code = "sample_code";
  event.message = std::move(message);
  event.fields = {{"route", "10.0.0.0/8"}};
  return event;
}

std::string read_file(const std::filesystem::path &path) {
  std::ifstream input(path);
  std::ostringstream output;
  output << input.rdbuf();
  return output.str();
}

bool expect(bool condition, const std::string &message) {
  if (condition) {
    return true;
  }
  std::cerr << "EXPECT FAILED: " << message << '\n';
  return false;
}

} // namespace

int main() {
  using exv::observability::LogLevel;

  bool ok = true;
  const auto temp_root = std::filesystem::temp_directory_path() /
                         "exv-platform-log-sink-test";
  std::filesystem::remove_all(temp_root);
  std::filesystem::create_directories(temp_root);

  {
    const auto log_path = temp_root / "nested" / "exv.log";
    int sync_count = 0;
    std::string synced_path;
    ecnuvpn::platform::logging::FileLogSink sink(
        log_path.string(), [&](const std::string &path) {
          ++sync_count;
          synced_path = path;
          return true;
        });

    sink.write(make_event(LogLevel::Info, "file sink message"));
    sink.flush();

    const auto content = read_file(log_path);
    ok &= expect(content.find("[INFO]") != std::string::npos,
                 "file sink should render uppercase level");
    ok &= expect(content.find("file sink message") != std::string::npos,
                 "file sink should write message");
    ok &= expect(content.find("route=10.0.0.0/8") != std::string::npos,
                 "file sink should render fields");
    ok &= expect(sync_count == 1 && synced_path == log_path.string(),
                 "file sink should call sync_owner callback");
  }

  {
    std::ostringstream output;
    ecnuvpn::platform::logging::StdoutLogSink sink(output);

    auto event = make_event(LogLevel::Error, "stdout sink message");
    event.fields.push_back({"attempt", "2"});
    sink.write(event);

    const auto parsed = nlohmann::json::parse(output.str());
    ok &= expect(parsed.value("event", "") == "log",
                 "stdout sink should emit log event envelope");
    const auto &data = parsed.at("data");
    ok &= expect(data.value("level", "") == "ERROR",
                 "stdout sink should render uppercase level");
    ok &= expect(data.value("message", "") == "stdout sink message",
                 "stdout sink should preserve message");
    ok &= expect(data.value("component", "") == "platform-test",
                 "stdout sink should preserve component");
    ok &= expect(data.value("code", "") == "sample_code",
                 "stdout sink should preserve code");
    ok &= expect(data.at("fields").value("attempt", "") == "2",
                 "stdout sink should render fields object");
  }

  {
    const auto log_path = temp_root / "redaction-policy.log";
    ecnuvpn::platform::logging::FileLogSink sink(log_path.string());
    auto event = make_event(LogLevel::Warn, "redaction policy");
    event.fields.push_back({"password", "secret-value"});
    sink.write(event);

    const auto content = read_file(log_path);
    ok &= expect(content.find("password=secret-value") != std::string::npos,
                 "file sink should not apply producer redaction policy");
  }

  std::filesystem::remove_all(temp_root);

  if (!ok) {
    std::cerr << "platform_log_sink_test: FAILED\n";
    return 1;
  }

  std::cout << "platform_log_sink_test: all assertions passed\n";
  return 0;
}
