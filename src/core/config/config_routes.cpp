#include "core/config/config.hpp"
#include "observability/log_facade.hpp"
#include "platform/common/logging/log_runtime.hpp"
#include "cli/console.hpp"

#include <algorithm>
#include <iostream>

namespace exv {
namespace config {

// ── Route management ────────────────────────────────────────────

bool add_route(Config &cfg, const std::string &route) {
  if (std::find(cfg.routes.begin(), cfg.routes.end(), route) !=
      cfg.routes.end()) {
    cli::print_warning("Route already exists: " + route);
    return false;
  }
  cfg.routes.push_back(route);
  save(cfg);
  cli::print_success("Route added: " + route);
  exv::observability::LogFacade::info("Route added: " + route);
  return true;
}

bool remove_route(Config &cfg, const std::string &route) {
  auto it = std::find(cfg.routes.begin(), cfg.routes.end(), route);
  if (it == cfg.routes.end()) {
    cli::print_error("Route not found: " + route);
    return false;
  }
  cfg.routes.erase(it);
  save(cfg);
  cli::print_success("Route removed: " + route);
  exv::observability::LogFacade::info("Route removed: " + route);
  return true;
}

void list_routes(const Config &cfg) {
  cli::print_header("VPN Routes");
  if (cfg.routes.empty()) {
    cli::print_warning("No routes configured.");
    return;
  }
  std::cout << "  Total: " << cfg.routes.size() << " routes" << std::endl
            << std::endl;
  for (size_t i = 0; i < cfg.routes.size(); ++i)
    std::cout << "  " << cli::GREEN << (i + 1) << "." << cli::RESET << " "
              << cfg.routes[i] << std::endl;
  std::cout << std::endl;
}

} // namespace config
} // namespace exv
