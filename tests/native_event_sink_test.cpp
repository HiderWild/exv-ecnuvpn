#include "vpn_engine/event_sink.hpp"

#include <filesystem>
#include <fstream>
#include <iostream>
#include <string>
#include <vector>

#include <nlohmann/json.hpp>

namespace {

bool expect(bool condition, const char *message) {
  if (condition)
    return true;

  std::cerr << "EXPECT FAILED: " << message << std::endl;
  return false;
}

std::string read_file_bytes(const std::filesystem::path &path) {
  std::ifstream in(path, std::ios::binary);
  if (!in.is_open())
    return {};

  std::string data;
  in.seekg(0, std::ios::end);
  data.resize(static_cast<size_t>(in.tellg()));
  in.seekg(0, std::ios::beg);
  in.read(data.data(), static_cast<std::streamsize>(data.size()));
  return data;
}

std::vector<std::string> split_lines(const std::string &s) {
  std::vector<std::string> lines;
  std::string cur;
  for (char c : s) {
    if (c == '\n') {
      lines.push_back(cur);
      cur.clear();
      continue;
    }
    cur.push_back(c);
  }
  if (!cur.empty())
    lines.push_back(cur);
  return lines;
}

} // namespace

int main() {
  bool ok = true;

  const auto tmp = std::filesystem::temp_directory_path();
  const auto path = tmp / "ecnuvpn_native_event_sink_test.jsonl";
  {
    std::error_code ec;
    std::filesystem::remove(path, ec);
  }

  ecnuvpn::vpn_engine::JsonLinesEventSink sink(path);

  // Round-trip exact UTF-8 text (no console codepage dependency).
  const std::string expected_bytes = "\xE8\xBF\x9E\xE6\x8E\xA5\xE6\x88\x90\xE5\x8A\x9F";
  const std::string expected = expected_bytes;
  ok = expect(expected == expected_bytes,
              "expected must be UTF-8 bytes for 连接成功") &&
       ok;

  const std::string msg = expected;

  ecnuvpn::vpn_engine::VpnEngineEvent event;
  event.type = "auth";
  event.level = "info";
  event.message = msg;
  event.fields["stage"] = "cookie";
  event.fields["user"] = "alice";
  sink.emit(event);

  ecnuvpn::vpn_engine::VpnEngineEvent err;
  err.type = "auth";
  err.level = "error";
  err.message = msg;
  err.fields["code"] = "auth_failed";
  err.fields["detail"] = "invalid_password";
  sink.emit(err);

  const std::string bytes = read_file_bytes(path);
  ok = expect(!bytes.empty(), "sink should write a non-empty file") && ok;
  ok = expect(bytes.find(expected_bytes) != std::string::npos,
              "serialized JSONL should contain raw UTF-8 bytes for 连接成功") &&
       ok;

  const auto lines = split_lines(bytes);
  ok = expect(lines.size() == 2, "sink should write exactly one JSON object per line") &&
       ok;

  // Events must serialize as valid UTF-8 JSON objects.
  nlohmann::json j0;
  nlohmann::json j1;
  try {
    j0 = nlohmann::json::parse(lines[0]);
    j1 = nlohmann::json::parse(lines[1]);
  } catch (const std::exception &e) {
    std::cerr << "JSON parse failed: " << e.what() << std::endl;
    return 1;
  }

  ok = expect(j0.value("message", std::string()) == expected,
              "message should round-trip exactly (连接成功)") &&
       ok;
  ok = expect(j0.contains("fields") && j0["fields"].is_object(),
              "event fields must be structured key/value pairs") &&
       ok;
  ok = expect(j0["fields"].value("stage", std::string()) == "cookie",
              "event fields should include stage") &&
       ok;

  ok = expect(j1.value("level", std::string()) == "error",
              "second event should be an error event") &&
       ok;
  ok = expect(j1.value("message", std::string()) == expected,
              "message should round-trip exactly (连接成功)") &&
       ok;
  ok = expect(j1.contains("fields") && j1["fields"].is_object() &&
                  j1["fields"].contains("code"),
              "error events must include code") &&
       ok;
  ok = expect(j1["fields"].value("code", std::string()) == "auth_failed",
              "error code should round-trip") &&
       ok;

  return ok ? 0 : 1;
}
