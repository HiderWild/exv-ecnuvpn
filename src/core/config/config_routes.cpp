#include "core/config/config.hpp"
#include "logger.hpp"
#include "utils.hpp"

#include <algorithm>
#include <iostream>

namespace ecnuvpn {
namespace config {

// ── Route management ────────────────────────────────────────────

bool add_route(Config &cfg, const std::string &route) {
  if (std::find(cfg.routes.begin(), cfg.routes.end(), route) !=
      cfg.routes.end()) {
    utils::print_warning("Route already exists: " + route);
    return false;
  }
  cfg.routes.push_back(route);
  save(cfg);
  utils::print_success("Route added: " + route);
  logger::info("Route added: " + route);
  return true;
}

bool remove_route(Config &cfg, const std::string &route) {
  auto it = std::find(cfg.routes.begin(), cfg.routes.end(), route);
  if (it == cfg.routes.end()) {
    utils::print_error("Route not found: " + route);
    return false;
  }
  cfg.routes.erase(it);
  save(cfg);
  utils::print_success("Route removed: " + route);
  logger::info("Route removed: " + route);
  return true;
}

void list_routes(const Config &cfg) {
  utils::print_header("VPN Routes");
  if (cfg.routes.empty()) {
    utils::print_warning("No routes configured.");
    return;
  }
  std::cout << "  Total: " << cfg.routes.size() << " routes" << std::endl
            << std::endl;
  for (size_t i = 0; i < cfg.routes.size(); ++i)
    std::cout << "  " << utils::GREEN << (i + 1) << "." << utils::RESET << " "
              << cfg.routes[i] << std::endl;
  std::cout << std::endl;
}

} // namespace config
} // namespace ecnuvpn
