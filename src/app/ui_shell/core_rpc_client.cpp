#include "app/ui_shell/core_rpc_client.hpp"

#include "cli/pipe_client.hpp"
#include "platform/common/core_resolver.hpp"
#include "platform/common/process_utils.hpp"
#include "runtime/runtime_context.hpp"

#include <nlohmann/json.hpp>

#ifdef _WIN32
#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <windows.h>
#else
#include <unistd.h>
#include <sys/types.h>
#endif

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

nlohmann::json payload_json_to_value(const std::string &payload_json) {
  if (payload_json.empty()) {
    return nlohmann::json::object();
  }
  return nlohmann::json::parse(payload_json);
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

CoreRpcClient::CoreRpcClient(CoreRpcTransport &transport,
                             CoreRpcWireMode wire_mode)
    : transport_(transport), wire_mode_(wire_mode) {}

CoreRpcResponse CoreRpcClient::invoke(const CoreRpcRequest &request) {
  std::string request_line;
  try {
    request_line = wire_mode_ == CoreRpcWireMode::Native
                       ? serialize_core_rpc_request(request)
                       : serialize_desktop_rpc_request(request);
  } catch (const nlohmann::json::exception &error) {
    return transport_error(request, "protocol_error", error.what());
  }

  if (!transport_.write_line(request_line)) {
    return transport_error(request, "transport_closed",
                           "Core RPC transport is closed");
  }

  for (;;) {
    std::string response_line;
    if (!buffered_response_lines_.empty()) {
      response_line = buffered_response_lines_.front();
      buffered_response_lines_.pop_front();
    } else if (!transport_.read_line(response_line)) {
      return transport_error(request, "transport_closed",
                             "Core RPC transport is closed");
    }

    try {
      const auto parsed = nlohmann::json::parse(response_line);
      if (parsed.is_object() && parsed.contains("event")) {
        if (event_handler_) {
          event_handler_(parse_core_rpc_event_line(response_line));
        }
        continue;
      }
      return parse_core_rpc_line(response_line);
    } catch (const nlohmann::json::exception &error) {
      return transport_error(request, "protocol_error", error.what());
    }
  }
}

void CoreRpcClient::set_event_handler(CoreRpcEventHandler handler) {
  event_handler_ = std::move(handler);
}

void CoreRpcClient::pump_events() {
  for (;;) {
    std::string line;
    if (!transport_.read_available_line(line)) {
      return;
    }

    try {
      const auto parsed = nlohmann::json::parse(line);
      if (parsed.is_object() && parsed.contains("event")) {
        if (event_handler_) {
          event_handler_(parse_core_rpc_event_line(line));
        }
        continue;
      }
      buffered_response_lines_.push_back(line);
    } catch (const nlohmann::json::exception &) {
      buffered_response_lines_.push_back(line);
    }
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

std::string serialize_desktop_rpc_request(const CoreRpcRequest &request) {
  nlohmann::ordered_json out;
  out["id"] = request_id_to_int(request.request_id);
  out["action"] = request.action;
  out["payload"] = payload_json_to_value(request.payload_json);
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
  out.data_json = parsed.contains("data") ? parsed.at("data").dump()
                                      : nlohmann::json::object().dump();
  return out;
}

exv::core::lifecycle::CoreResolverDeps make_pipe_resolver_deps() {
  using namespace exv::core::lifecycle;
  CoreResolverDeps deps;

  deps.try_connect_ipc = [](const std::string &ipc_path) {
    return exv::cli::PipeClient::probe(ipc_path);
  };

  deps.send_ipc_request = [](const std::string &ipc_path,
                             const std::string &request_line) -> std::string {
    exv::cli::PipeClient client;
    if (!client.connect(ipc_path)) {
      return {};
    }
    std::string response = client.send_request(request_line);
    client.disconnect();
    return response;
  };

  deps.disconnect_ipc = [] {};

  deps.is_pid_alive = [](int pid) -> bool {
#ifdef _WIN32
    HANDLE h = OpenProcess(SYNCHRONIZE | PROCESS_QUERY_LIMITED_INFORMATION,
                           FALSE, static_cast<DWORD>(pid));
    if (!h) return false;
    DWORD code = 0;
    if (!GetExitCodeProcess(h, &code) || code != STILL_ACTIVE) {
      CloseHandle(h);
      return false;
    }
    CloseHandle(h);
    return true;
#else
    return kill(pid, 0) == 0;
#endif
  };

  deps.get_frontend_executable_path = []() {
    return ecnuvpn::platform::get_executable_path();
  };

  deps.run_command_output = [](const std::string &cmd) {
    return ecnuvpn::platform::run_command_output(cmd);
  };

  deps.get_state_dir = []() {
    return ecnuvpn::runtime::paths().state_dir;
  };

  deps.get_home_dir = []() {
    return ecnuvpn::runtime::paths().home;
  };

  deps.get_env_var = [](const std::string &name) -> std::string {
    const char *val = std::getenv(name.c_str());
    return val ? std::string(val) : std::string();
  };

  return deps;
}

} // namespace ecnuvpn::ui_shell
