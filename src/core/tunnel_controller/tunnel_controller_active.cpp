#include "tunnel_controller_active.hpp"

#include <atomic>

namespace exv::core {

static std::atomic<bool> g_tunnel_controller_active{false};

bool is_tunnel_controller_active() {
    return g_tunnel_controller_active.load();
}

void set_tunnel_controller_active(bool active) {
    g_tunnel_controller_active.store(active);
}

} // namespace exv::core
