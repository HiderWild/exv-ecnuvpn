int run_tunnel_script(const TunnelScriptContext &context) {
  const std::string reason = env_value("reason");
  const std::string &ready_path = context.route_ready_path;
  debug_log(ready_path, "native script reason=" + reason);

  if (reason == "pre-init") {
    delete_ready_file(ready_path);
    return 0;
  }

  if (reason == "disconnect") {
    for (const auto &ip : context.server_route_exceptions)
      run_exit(ready_path,
               "route.exe delete " + ip + " mask 255.255.255.255");
    for (const auto &cidr : context.custom_routes) {
      auto [network, mask] = cidr_to_network_and_mask(cidr);
      run_exit(ready_path, "route.exe delete " + network + " mask " + mask);
    }
    delete_ready_file(ready_path);
    return 0;
  }

  if (reason == "reconnect" || reason == "attempt-reconnect") {
    return 0;
  }

  if (reason != "connect") {
    return 0;
  }

  const std::string tunidx = env_value("TUNIDX");
  const std::string tundev = env_value("TUNDEV", tunidx);
  const std::string internal_ip = env_value("INTERNAL_IP4_ADDRESS");
  const std::string netmask = env_value("INTERNAL_IP4_NETMASK",
                                        "255.255.255.255");
  const std::string mtu = env_value("INTERNAL_IP4_MTU");

  debug_log(ready_path, "connect TUNIDX=" + tunidx + " TUNDEV=" + tundev +
                            " IP=" + internal_ip);

  return configure_tunnel_network(context, tunidx, tundev, internal_ip,
                                  netmask, mtu)
             ? 0
             : 1;
}

OpenconnectLogConfigureResult
configure_from_openconnect_log(const TunnelScriptContext &context,
                               const std::string &log_path) {
  if (context.vpn_engine == "native")
    return {false, "native_log_scraping_disabled"};

  std::ifstream in(log_path);
  if (!in.is_open())
    return {false, ""};
  std::string content((std::istreambuf_iterator<char>(in)),
                      std::istreambuf_iterator<char>());

  openconnect_log::Evidence evidence = openconnect_log::parse_evidence(content);
  if (evidence.auth_failed || !evidence.has_tunnel_metadata) {
    return {false, ""};
  }

  debug_log(context.route_ready_path,
            "fallback from log TUNIDX=" + evidence.if_index +
                " TUNDEV=" + evidence.adapter + " IP=" +
                evidence.internal_ip);
  bool ok = configure_tunnel_network(context, evidence.if_index,
                                     evidence.adapter, evidence.internal_ip,
                                     "255.255.240.0", "");
  return {ok, ""};
}

void cleanup_tunnel_routes(const TunnelScriptContext &) {}
