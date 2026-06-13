#include "core/vpn/vpn.hpp"
#include "app_api.hpp"
#include "config.hpp"
#include "helper.hpp"
#include "logger.hpp"
#include "log_renderer.hpp"
#include "runtime/runtime_context.hpp"
#include "utils.hpp"
#include "vpn_engine/native_engine.hpp"

#include <iostream>

namespace ecnuvpn {
namespace vpn {

int start(const Config &cfg, int retry_limit) {
  // Enable log rendering for CLI mode - subscribes to LogEventBus
  ecnuvpn::LogRenderer log_renderer;
  
  logger::info("VPN CLI: Connection starting - server=" + cfg.server + 
               " username=" + cfg.username + " engine=" + cfg.vpn_engine);
  
  utils::print_header("EXV Starting");

  if (cfg.vpn_engine == "native") {
    logger::info("VPN CLI: Validating native engine configuration");
    auto validation = vpn_engine::validate_native_config(cfg);
    if (!validation.ok) {
      utils::print_error(validation.message);
      logger::error("VPN CLI: Native engine validation failed - code=" + 
                    validation.code + " message=" + validation.message);
      return 1;
    }
    logger::info("VPN CLI: Native engine configuration validated successfully");
  }

  nlohmann::json payload;
  payload["password"] = config::get_plaintext_password(cfg);
  payload["retry_limit"] = retry_limit;
  
  logger::info("VPN CLI: Calling app_api::handle_action(vpn.connect)");
  nlohmann::json result = app_api::handle_action("vpn.connect", payload);

  if (result.is_object() && result.value("ok", true) == false) {
    std::string msg = result.value("error", std::string("Connection failed"));
    std::string code = result.value("code", std::string());
    utils::print_error(msg);
    logger::error("VPN CLI: Connection failed - code=" + code + " error=" + msg);
    return 1;
  }

  logger::info("VPN CLI: Connection initiated successfully");
  utils::print_success("VPN connection initiated");
  return 0;
}

int stop() {
  nlohmann::json result = app_api::handle_action("vpn.disconnect", nlohmann::json::object());
  if (result.is_object() && result.value("ok", true) == false) {
    utils::print_error(result.value("error", std::string("Disconnect failed")));
    return 1;
  }
  utils::print_success("VPN disconnected");
  return 0;
}

int status() {
  nlohmann::json result = app_api::handle_action("status.get", nlohmann::json::object());

  if (result.is_object() && result.value("ok", true) == false) {
    utils::print_error("Failed to get VPN status");
    return 1;
  }

  bool connected = result.value("connected", false);
  if (connected) {
    std::cout << utils::GREEN << utils::BOLD << "  ● VPN is RUNNING" << utils::RESET << std::endl;
    std::cout << std::endl;
    std::cout << "  Server         : " << result.value("server", std::string()) << std::endl;
    std::cout << "  Interface      : " << result.value("interface", std::string()) << std::endl;
    std::cout << "  Network Ready  : " << (result.value("network_ready", false) ? "yes" : "no") << std::endl;
  } else {
    std::cout << utils::RED << utils::BOLD << "  ● VPN is NOT RUNNING" << utils::RESET << std::endl;
  }
  std::cout << std::endl;
  return 0;
}

} // namespace vpn
} // namespace ecnuvpn
