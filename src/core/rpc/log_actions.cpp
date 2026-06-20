#include "core/rpc/log_actions.hpp"

#include "platform/common/file_system.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "platform/common/runtime_paths.hpp"
#include "runtime/runtime_context.hpp"

#include <nlohmann/json.hpp>

#include <cstdint>
#include <deque>
#include <fstream>
#include <limits>
#include <string>

namespace exv::core_api {
namespace {

using json = nlohmann::json;
constexpr std::size_t kMaxLogLineChars = 1024;
constexpr std::size_t kMaxLogResponsePayloadBytes = 6000;

json parse_payload(const RpcRequest &req) {
  if (req.payload_json.empty()) {
    return json::object();
  }
  return json::parse(req.payload_json);
}

RpcResponse success_response(const json &payload) {
  RpcResponse resp;
  resp.success = true;
  resp.payload_json = payload.dump();
  return resp;
}

RpcResponse error_response(const std::string &code, const std::string &message) {
  RpcResponse resp;
  resp.success = false;
  resp.error_code = code;
  resp.error_message = message;
  return resp;
}

void prepare_log_runtime() {
  exv::platform::ensure_dir(exv::platform::get_config_dir());
  exv::platform::logging::configure_default_logging(false);
}

std::string log_safe_text(std::string value) {
  for (char &ch : value) {
    if (static_cast<unsigned char>(ch) < 0x20 && ch != '\t') {
      ch = ' ';
    }
  }
  if (value.size() > kMaxLogLineChars) {
    value.resize(kMaxLogLineChars);
    value += " ... [truncated]";
  }
  return value;
}

std::int64_t json_int64_or(const json &payload, const char *key,
                           std::int64_t fallback) {
  if (!payload.contains(key)) {
    return fallback;
  }
  const auto &value = payload.at(key);
  if (value.is_number_integer()) {
    return value.get<std::int64_t>();
  }
  if (value.is_number_unsigned()) {
    const auto unsigned_value = value.get<std::uint64_t>();
    if (unsigned_value >
        static_cast<std::uint64_t>(std::numeric_limits<std::int64_t>::max())) {
      return std::numeric_limits<std::int64_t>::max();
    }
    return static_cast<std::int64_t>(unsigned_value);
  }
  return fallback;
}

int requested_log_limit(const json &payload) {
  constexpr int kDefaultLogEntries = 100;
  constexpr int kMaxLogEntriesPerResponse = 500;

  std::int64_t limit = json_int64_or(payload, "limit",
                                     json_int64_or(payload, "lines",
                                                   kDefaultLogEntries));
  if (limit < 1) {
    limit = 1;
  }
  if (limit > kMaxLogEntriesPerResponse) {
    limit = kMaxLogEntriesPerResponse;
  }
  return static_cast<int>(limit);
}

std::int64_t requested_after_seq(const json &payload) {
  std::int64_t after_seq = json_int64_or(payload, "after_seq", 0);
  if (after_seq < 0) {
    after_seq = 0;
  }
  return after_seq;
}

json make_log_entry(std::int64_t seq, const std::string &line) {
  return {{"seq", seq},
          {"timestamp", ""},
          {"level", "info"},
          {"message", log_safe_text(line)}};
}

std::size_t log_entry_wire_size(const json &entry) {
  return entry.dump().size() + 1;
}

json read_log_lines(const json &payload) {
  prepare_log_runtime();

  const std::string log_path = exv::runtime::paths().log_path;
  const int max_lines = requested_log_limit(payload);
  const std::int64_t after_seq = requested_after_seq(payload);
  const std::string filter = payload.value("filter", std::string());

  std::deque<json> tail_entries;
  std::deque<std::size_t> tail_entry_sizes;
  std::size_t tail_payload_bytes = 2;
  json incremental_entries = json::array();
  std::size_t incremental_payload_bytes = 2;
  std::ifstream ifs(log_path);
  std::string line;
  std::int64_t seq = 0;
  while (std::getline(ifs, line)) {
    ++seq;
    if (!line.empty() && line.back() == '\r') {
      line.pop_back();
    }
    if (!filter.empty() && line.find(filter) == std::string::npos) {
      continue;
    }

    const auto entry = make_log_entry(seq, line);
    const auto entry_size = log_entry_wire_size(entry);
    tail_entries.push_back(entry);
    tail_entry_sizes.push_back(entry_size);
    tail_payload_bytes += entry_size;
    while (tail_entries.size() > 1 &&
           (tail_entries.size() > static_cast<std::size_t>(max_lines) ||
            tail_payload_bytes > kMaxLogResponsePayloadBytes)) {
      tail_payload_bytes -= tail_entry_sizes.front();
      tail_entry_sizes.pop_front();
      tail_entries.pop_front();
    }

    if (after_seq > 0 && seq > after_seq &&
        incremental_entries.size() < static_cast<std::size_t>(max_lines)) {
      if (incremental_entries.empty() ||
          incremental_payload_bytes + entry_size <= kMaxLogResponsePayloadBytes) {
        incremental_entries.push_back(entry);
        incremental_payload_bytes += entry_size;
      }
    }
  }

  if (after_seq > 0 && after_seq <= seq) {
    return incremental_entries;
  }

  json lines = json::array();
  for (const auto &entry : tail_entries) {
    lines.push_back(entry);
  }
  return lines;
}

json clear_log_file() {
  prepare_log_runtime();

  const std::string log_path = exv::runtime::paths().log_path;
  std::ofstream out(log_path, std::ios::out | std::ios::trunc);
  if (!out.is_open()) {
    return {{"ok", false},
            {"code", "log_clear_failed"},
            {"message", "Unable to clear the runtime log."}};
  }
  out.close();
  return {{"ok", true},
          {"cleared", true},
          {"logDirectory", "<redacted>"},
          {"filesRemoved", 0}};
}

} // namespace

void LogActions::register_handlers(AppRpcDispatcher &dispatcher) {
  dispatcher.register_handler("logs.list",
                              [this](const RpcRequest &req) { return list(req); });
  dispatcher.register_handler("logs.clear",
                              [this](const RpcRequest &req) { return clear(req); });
}

RpcResponse LogActions::list(const RpcRequest &req) {
  try {
    return success_response(read_log_lines(parse_payload(req)));
  } catch (const std::exception &e) {
    return error_response("invalid_payload", e.what());
  }
}

RpcResponse LogActions::clear(const RpcRequest &req) {
  try {
    (void)parse_payload(req);
    const auto result = clear_log_file();
    if (!result.value("ok", false)) {
      return error_response(result.value("code", "log_clear_failed"),
                            result.value("message", "Unable to clear the runtime log."));
    }
    return success_response(result);
  } catch (const std::exception &e) {
    return error_response("invalid_payload", e.what());
  }
}

} // namespace exv::core_api
