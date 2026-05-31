#include "platform/common/tunnel_script.hpp"

#include "utils.hpp"

#include <sstream>
#include <string>

namespace ecnuvpn {
namespace platform {

std::string generate_tunnel_script(const TunnelScriptContext &context) {
  std::ostringstream ss;

  ss << "#!/bin/bash\n";
  ss << "\n";
  ss << "# =================================================================\n";
  ss << "# ECNU-VPN Split Tunnel Script (Auto-generated)\n";
  ss << "# Do NOT edit manually - regenerated on each VPN start\n";
  ss << "# =================================================================\n";
  ss << "\n";
  ss << "READY_FILE=\"" << context.route_ready_path << "\"\n";
  if (context.has_runtime_owner) {
    ss << "OWNER_UID=\"" << context.runtime_owner_uid << "\"\n";
    ss << "OWNER_GID=\"" << context.runtime_owner_gid << "\"\n";
  }
  ss << "\n";

  ss << "CUSTOM_ROUTES=\"";
  for (std::size_t i = 0; i < context.custom_routes.size(); ++i) {
    if (i > 0)
      ss << " ";
    ss << context.custom_routes[i];
  }
  ss << "\"\n";
  ss << "SERVER_EXCEPTIONS=\"";
  for (std::size_t i = 0; i < context.server_route_exceptions.size(); ++i) {
    if (i > 0)
      ss << " ";
    ss << context.server_route_exceptions[i];
  }
  ss << "\"\n";
  ss << "\n";
  ss << "now_ms() {\n";
  ss << "    perl -MTime::HiRes=time -e 'printf \"%.0f\", time()*1000' 2>/dev/null || date +%s000\n";
  ss << "}\n";
  ss << "SCRIPT_START_MS=$(now_ms)\n";
  ss << "SCRIPT_LAST_MS=$SCRIPT_START_MS\n";
  ss << "timing_mark() {\n";
  ss << "    NOW_MS=$(now_ms)\n";
  ss << "    DELTA_MS=$((NOW_MS - SCRIPT_LAST_MS))\n";
  ss << "    TOTAL_MS=$((NOW_MS - SCRIPT_START_MS))\n";
  ss << "    SCRIPT_LAST_MS=$NOW_MS\n";
  ss << "    echo \"[EXV-TIMING] scope=tunnel.linux stage=$1 delta_ms=$DELTA_MS total_ms=$TOTAL_MS\"\n";
  ss << "}\n\n";

  ss << "delete_ready_file() {\n";
  ss << "    rm -f \"$READY_FILE\"\n";
  ss << "}\n\n";

  ss << "cleanup_routes() {\n";
  ss << "    VPN_GW=\"$VPNGATEWAY\"\n";
  ss << "    if [ -n \"$VPN_GW\" ] && [ \"${VPN_GW%%:*}\" = \"$VPN_GW\" ]; then\n";
  ss << "        ip route del \"$VPN_GW\" >/dev/null 2>&1\n";
  ss << "    fi\n";
  ss << "    for ip in $SERVER_EXCEPTIONS; do\n";
  ss << "        ip route del \"$ip\" >/dev/null 2>&1\n";
  ss << "    done\n";
  ss << "    for route in $CUSTOM_ROUTES; do\n";
  ss << "        ip route del \"$route\" >/dev/null 2>&1\n";
  ss << "    done\n";
  ss << "}\n\n";

  ss << "case \"$reason\" in\n";
  ss << "    pre-init)\n";
  ss << "        timing_mark pre_init\n";
  ss << "        delete_ready_file\n";
  ss << "        exit 0\n";
  ss << "        ;;\n";
  ss << "    disconnect|reconnect|attempt-reconnect)\n";
  ss << "        timing_mark cleanup_start\n";
  ss << "        cleanup_routes\n";
  ss << "        delete_ready_file\n";
  ss << "        timing_mark cleanup_done\n";
  ss << "        exit 0\n";
  ss << "        ;;\n";
  ss << "    connect)\n";
  ss << "        ;;\n";
  ss << "    *)\n";
  ss << "        exit 0\n";
  ss << "        ;;\n";
  ss << "esac\n\n";

  ss << "echo \">>> [VPN] Connection established, configuring network...\"\n";
  ss << "echo \">>> [VPN] Interface: $TUNDEV | Internal IP: $INTERNAL_IP4_ADDRESS\"\n\n";
  ss << "timing_mark connect_start\n\n";

  ss << "# Activate virtual interface\n";
  ss << "ip addr add \"$INTERNAL_IP4_ADDRESS/32\" dev \"$TUNDEV\" >/dev/null 2>&1\n";
  ss << "ip link set \"$TUNDEV\" up >/dev/null 2>&1\n";
  ss << "if [ $? -ne 0 ]; then\n";
  ss << "    echo \">>> [VPN] Failed to activate interface: $TUNDEV\"\n";
  ss << "    exit 1\n";
  ss << "fi\n\n";
  ss << "timing_mark activate_interface\n\n";

  if (!context.server_route_exceptions.empty()) {
    ss << "DEFAULT_GATEWAY=$(ip route show default 0.0.0.0/0 2>/dev/null | awk '{print $3; exit}')\n";
    ss << "DEFAULT_INTERFACE=$(ip route show default 0.0.0.0/0 2>/dev/null | awk '{print $5; exit}')\n";
    ss << "if [ -n \"$DEFAULT_GATEWAY\" ] && [ -n \"$DEFAULT_INTERFACE\" ]; then\n";
    for (const auto &server_ip : context.server_route_exceptions) {
      ss << "    ip route del \"" << server_ip << "\" >/dev/null 2>&1\n";
      ss << "    ip route add \"" << server_ip
         << "\" via \"$DEFAULT_GATEWAY\" >/dev/null 2>&1\n";
      ss << "    if [ $? -eq 0 ]; then\n";
      ss << "        echo \"  [+] Server route preserved: " << server_ip
         << " via $DEFAULT_INTERFACE\"\n";
      ss << "    else\n";
      ss << "        echo \"  [-] Server route warning: " << server_ip
         << " (failed to preserve upstream path)\"\n";
      ss << "    fi\n";
    }
    ss << "fi\n\n";
  }
  ss << "timing_mark preserve_server_routes\n\n";

  ss << "# Split tunnel routes\n";
  ss << "echo \">>> [VPN] Adding split tunnel routes...\"\n\n";

  for (const auto &route : context.custom_routes) {
    ss << "ip route del \"" << route << "\" >/dev/null 2>&1\n";
    ss << "ip route add \"" << route << "\" dev \"$TUNDEV\" >/dev/null 2>&1\n";
    ss << "if [ $? -eq 0 ]; then\n";
    ss << "    echo \"  [+] Route added: " << route << "\"\n";
    ss << "else\n";
    ss << "    echo \"  [-] Route warning: " << route
       << " (failed to refresh)\"\n";
    ss << "fi\n\n";
  }
  ss << "timing_mark add_split_routes\n\n";

  ss << "printf '%s\\n%s\\n' \"$TUNDEV\" \"$INTERNAL_IP4_ADDRESS\" > \"$READY_FILE\"\n";
  ss << "if [ $? -ne 0 ]; then\n";
  ss << "    echo \">>> [VPN] Failed to write route-ready marker.\"\n";
  ss << "    exit 1\n";
  ss << "fi\n";
  if (context.has_runtime_owner) {
    ss << "chown \"$OWNER_UID\":\"$OWNER_GID\" \"$READY_FILE\" >/dev/null 2>&1\n";
    ss << "chmod 0644 \"$READY_FILE\" >/dev/null 2>&1\n";
  }
  ss << "\n";
  ss << "timing_mark write_route_ready\n";
  ss << "echo \">>> [VPN] Network configuration complete!\"\n";
  ss << "echo \">>> [Tip] Campus traffic via VPN, other traffic via default route.\"\n\n";
  ss << "timing_mark finish_ok\n\n";
  ss << "exit 0\n";

  return ss.str();
}

int run_tunnel_script(const TunnelScriptContext &) { return 0; }

OpenconnectLogConfigureResult
configure_from_openconnect_log(const TunnelScriptContext &context,
                               const std::string &) {
  if (context.vpn_engine == "native")
    return {false, "native_log_scraping_disabled"};
  return {false, ""};
}

void cleanup_tunnel_routes(const TunnelScriptContext &context) {
  for (const auto &route : context.custom_routes) {
    utils::run_command("ip route del " + utils::shell_quote(route) +
                       " >/dev/null 2>&1");
  }
  for (const auto &ip : context.server_route_exceptions) {
    utils::run_command("ip route del " + utils::shell_quote(ip) +
                       " >/dev/null 2>&1");
  }
}

} // namespace platform
} // namespace ecnuvpn
