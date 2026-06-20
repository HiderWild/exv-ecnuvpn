#include "config_actions.hpp"

#include <nlohmann/json.hpp>

#include <utility>

using json = nlohmann::json;

namespace exv::core_api {
namespace {

RpcResponse to_rpc_response(const exv::core::UseCaseResult &result) {
  RpcResponse resp;
  resp.success = result.success;
  if (result.success) {
    resp.payload_json = result.payload.dump();
  } else {
    resp.error_code = result.error_code;
    resp.error_message = result.error_message;
  }
  return resp;
}

exv::core::UseCaseResult invalid_payload(const std::exception &e) {
  return exv::core::UseCaseResult::fail("invalid_payload", e.what());
}

exv::core::UseCaseResult try_parse_payload(const RpcRequest& req) {
  if (req.payload_json.empty()) {
    return exv::core::UseCaseResult::ok(json::object());
  }
  try {
    return exv::core::UseCaseResult::ok(json::parse(req.payload_json));
  } catch (const std::exception& e) {
    return invalid_payload(e);
  }
}

} // namespace

ConfigActions::ConfigActions() = default;

ConfigActions::ConfigActions(std::string config_dir)
    : use_cases_(std::move(config_dir)) {}

void ConfigActions::register_handlers(AppRpcDispatcher& dispatcher) {
  dispatcher.register_handler("config.get",
      [this](const RpcRequest& req) { return get(req); });
  dispatcher.register_handler("config.save",
      [this](const RpcRequest& req) { return save(req); });

  dispatcher.register_handler("config.get_profile",
      [this](const RpcRequest& req) { return get_profile(req); });
  dispatcher.register_handler("config.save_profile",
      [this](const RpcRequest& req) { return save_profile(req); });

  dispatcher.register_handler("config.getAuth",
      [this](const RpcRequest& req) { return get_auth(req); });
  dispatcher.register_handler("config.saveAuth",
      [this](const RpcRequest& req) { return save_auth(req); });
  dispatcher.register_handler("config.getSettings",
      [this](const RpcRequest& req) { return get_settings(req); });
  dispatcher.register_handler("config.saveSettings",
      [this](const RpcRequest& req) { return save_settings(req); });
  dispatcher.register_handler("config.reset",
      [this](const RpcRequest& req) { return reset_config(req); });

  dispatcher.register_handler("key.status",
      [this](const RpcRequest& req) { return key_status(req); });
  dispatcher.register_handler("key.reset",
      [this](const RpcRequest& req) { return reset_key(req); });

  dispatcher.register_handler("config.import",
      [this](const RpcRequest& req) { return import_config(req); });
  dispatcher.register_handler("config.export",
      [this](const RpcRequest& req) { return export_config(req); });
}

RpcResponse ConfigActions::get(const RpcRequest& req) {
  return to_rpc_response(use_cases_.get_config());
}

RpcResponse ConfigActions::save(const RpcRequest& req) {
  try {
    auto payload = json::parse(req.payload_json);
    return to_rpc_response(use_cases_.save_config(payload));
  } catch (const std::exception& e) {
    return to_rpc_response(invalid_payload(e));
  }
}

RpcResponse ConfigActions::get_profile(const RpcRequest& req) {
  try {
    auto payload = json::parse(req.payload_json);
    return to_rpc_response(use_cases_.get_profile(payload));
  } catch (const std::exception& e) {
    return to_rpc_response(invalid_payload(e));
  }
}

RpcResponse ConfigActions::save_profile(const RpcRequest& req) {
  try {
    auto payload = json::parse(req.payload_json);
    return to_rpc_response(use_cases_.save_profile(payload));
  } catch (const std::exception& e) {
    return to_rpc_response(invalid_payload(e));
  }
}

RpcResponse ConfigActions::get_auth(const RpcRequest& req) {
  (void)req;
  return to_rpc_response(use_cases_.get_auth());
}

RpcResponse ConfigActions::save_auth(const RpcRequest& req) {
  try {
    auto payload = json::parse(req.payload_json);
    return to_rpc_response(use_cases_.save_auth(payload));
  } catch (const std::exception& e) {
    return to_rpc_response(invalid_payload(e));
  }
}

RpcResponse ConfigActions::get_settings(const RpcRequest& req) {
  (void)req;
  return to_rpc_response(use_cases_.get_settings());
}

RpcResponse ConfigActions::save_settings(const RpcRequest& req) {
  try {
    auto payload = json::parse(req.payload_json);
    return to_rpc_response(use_cases_.save_settings(payload));
  } catch (const std::exception& e) {
    return to_rpc_response(invalid_payload(e));
  }
}

RpcResponse ConfigActions::reset_config(const RpcRequest& req) {
  (void)req;
  return to_rpc_response(use_cases_.reset_config());
}

RpcResponse ConfigActions::key_status(const RpcRequest& req) {
  (void)req;
  return to_rpc_response(use_cases_.get_key_status());
}

RpcResponse ConfigActions::reset_key(const RpcRequest& req) {
  (void)req;
  return to_rpc_response(use_cases_.reset_key());
}

RpcResponse ConfigActions::list_routes(const RpcRequest& req) {
  (void)req;
  return to_rpc_response(use_cases_.list_routes());
}

RpcResponse ConfigActions::add_route(const RpcRequest& req) {
  try {
    auto payload = json::parse(req.payload_json);
    return to_rpc_response(use_cases_.add_route(payload));
  } catch (const std::exception& e) {
    return to_rpc_response(invalid_payload(e));
  }
}

RpcResponse ConfigActions::remove_route(const RpcRequest& req) {
  try {
    auto payload = json::parse(req.payload_json);
    return to_rpc_response(use_cases_.remove_route(payload));
  } catch (const std::exception& e) {
    return to_rpc_response(invalid_payload(e));
  }
}

RpcResponse ConfigActions::reset_routes(const RpcRequest& req) {
  (void)req;
  return to_rpc_response(use_cases_.reset_routes());
}

RpcResponse ConfigActions::import_config(const RpcRequest& req) {
  try {
    auto payload = json::parse(req.payload_json);
    return to_rpc_response(use_cases_.import_config(payload));
  } catch (const std::exception& e) {
    return to_rpc_response(invalid_payload(e));
  }
}

RpcResponse ConfigActions::export_config(const RpcRequest& req) {
  exv::core::UseCaseResult parsed = try_parse_payload(req);
  if (!parsed.success) {
    return to_rpc_response(parsed);
  }
  return to_rpc_response(use_cases_.export_config(parsed.payload));
}

} // namespace exv::core_api
