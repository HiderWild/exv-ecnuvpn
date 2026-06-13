#include "core/config/config.hpp"
#include "crypto.hpp"
#include "logger.hpp"
#include "utils.hpp"

#include <iomanip>
#include <iostream>

namespace ecnuvpn {
namespace config {

// ── Show ─────────────────────────────────────────────────────────

void show(const Config &cfg) {
  utils::print_header("EXV Configuration");
  std::cout << "  " << utils::BOLD << "Server" << utils::RESET << "       : "
            << cfg.server << std::endl;
  std::cout << "  " << utils::BOLD << "Username" << utils::RESET << "     : "
            << cfg.username << std::endl;
  std::cout << "  " << utils::BOLD << "Password" << utils::RESET << "     : "
            << (cfg.password.empty() ? std::string(utils::DIM) + "(not stored)"
                                     : std::string(utils::GREEN) + "(encrypted)")
            << utils::RESET << std::endl;
  std::cout << "  " << utils::BOLD << "MTU" << utils::RESET << "          : "
            << cfg.mtu << std::endl;
  std::cout << "  " << utils::BOLD << "User-Agent" << utils::RESET << "   : "
            << cfg.useragent << std::endl;
  std::cout << "  " << utils::BOLD << "DTLS" << utils::RESET << "         : "
            << (cfg.disable_dtls ? "disabled" : "enabled") << std::endl;
  std::cout << "  " << utils::BOLD << "Remember PW" << utils::RESET << "  : "
            << (cfg.remember_password ? "yes" : "no") << std::endl;
  std::cout << "  " << utils::BOLD << "Auto Reconnect" << utils::RESET << ": "
            << (cfg.auto_reconnect ? "yes" : "no") << std::endl;
  std::cout << "  " << utils::BOLD << "VPN Engine" << utils::RESET << "   : "
            << cfg.vpn_engine << std::endl;
  std::cout << "  " << utils::BOLD << "Runtime" << utils::RESET << "      : "
            << cfg.openconnect_runtime << std::endl;
  std::cout << "  " << utils::BOLD << "Tunnel Driver" << utils::RESET << ": "
            << cfg.windows_tunnel_driver << std::endl;
  std::cout << "  " << utils::BOLD << "Log File" << utils::RESET << "     : "
            << cfg.log_file << std::endl;
  std::cout << "  " << utils::BOLD << "Minimal Mode" << utils::RESET << " : "
            << (cfg.minimal_mode ? "yes" : "no") << std::endl;
  std::cout << std::endl;

  list_routes(cfg);
  key_show();
}

} // namespace config
} // namespace ecnuvpn
