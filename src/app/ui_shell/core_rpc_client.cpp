#include "app/ui_shell/core_rpc_client.hpp"

#include <nlohmann/json.hpp>

#include <charconv>
#include <system_error>
#include <utility>

namespace ecnuvpn::ui_shell {
namespace {

int request_id_to_int(const std::string &request_id) {
  if (request_id.empty()) {
    return 0;
  }
  int value = 0;
  const char *begin = request_id.data();
  const char *end = begin + request_id.size();
  const auto result = std::from_chars(begin, end, value);
  if (result.ec != std::errc{} || result.ptr != end) {
    return 0;
  }
  return value;
}

std::string payload_json_to_string(const nlohmann::json &value) {
  if (value.is_string()) {
    return value.get<std::string>();
  }
  return value.dump();
}

CoreRpcResponse transport_error(const CoreRpcRequest &request,
                                std::string code,
                                std::string message) {
  CoreRpcResponse response;
  response.id = request_id_to_int(request.request_id);
  response.request_id = request.request_id;
  response.ok = false;
  response.code = std::move(code);
  response.message = std::move(message);
  return response;
}

} // namespace

CoreRpcClient::CoreRpcClient(CoreRpcTransport &transport)
    : transport_(transport) {}

CoreRpcResponse CoreRpcClient::invoke(const CoreRpcRequest &request) {
  if (!transport_.write_line(serialize_core_rpc_request(request))) {
    return transport_error(request, "transport_closed",
                           "Core RPC transport is closed");
  }

  std::string response_line;
  if (!transport_.read_line(response_line)) {
    return transport_error(request, "transport_closed",
                           "Core RPC transport is closed");
  }

  try {
    return parse_core_rpc_line(response_line);
  } catch (const nlohmann::json::exception &error) {
    return transport_error(request, "protocol_error", error.what());
  }
}

std::string serialize_core_rpc_request(const CoreRpcRequest &request) {
  nlohmann::ordered_json out;
  out["action"] = request.action;
  out["payload_json"] = request.payload_json.empty()
                            ? nlohmann::json::object().dump()
                            : request.payload_json;
  out["request_id"] = request.request_id;
  return out.dump();
}

CoreRpcResponse parse_core_rpc_line(const std::string &line) {
  const auto parsed = nlohmann::json::parse(line);
  CoreRpcResponse out;

  if (parsed.contains("success")) {
    out.request_id = parsed.at("request_id").get<std::string>();
    out.id = request_id_to_int(out.request_id);
    out.ok = parsed.value("success", false);
    if (out.ok && parsed.contains("payload_json")) {
      out.data_json = payload_json_to_string(parsed.at("payload_json"));
    }
    out.code = parsed.value("error_code", "");
    out.message = parsed.value("error_message", "");
    return out;
  }

  out.id = parsed.value("id", 0);
  out.request_id = std::to_string(out.id);
  out.ok = parsed.value("ok", false);
  if (parsed.contains("data")) {
    out.data_json = parsed.at("data").dump();
  }
  out.code = parsed.value("code", "");
  out.message = parsed.value("message", "");
  return out;
}

CoreRpcEvent parse_core_rpc_event_line(const std::string &line) {
  const auto parsed = nlohmann::json::parse(line);
  CoreRpcEvent out;
  out.event = parsed.value("event", "");
  if (parsed.contains("data")) {
    out.data_json = parsed.at("data").dump();
  }
  return out;
}

} // namespace ecnuvpn::ui_shell
