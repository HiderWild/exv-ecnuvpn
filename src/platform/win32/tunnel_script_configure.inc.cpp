bool configure_tunnel_network(const TunnelScriptContext &context,
                              const std::string &tunidx,
                              const std::string &tundev,
                              const std::string &internal_ip,
                              const std::string &netmask,
                              const std::string &mtu) {
  TunnelTiming timing;
  const std::string &ready_path = context.route_ready_path;
  delete_ready_file(ready_path);

  if (tunidx.empty() || internal_ip.empty()) {
    debug_log(ready_path, "missing openconnect tunnel metadata");
    timing.finish(false, "reason=missing_metadata");
    return false;
  }

  const std::string adapter = tundev.empty() ? tunidx : tundev;
  const std::string if_index = is_numeric(tunidx) ? tunidx : "";
  const std::string address_target =
      is_numeric(adapter) ? adapter : "name=" + cmd_quote_arg(adapter);
  const std::string subinterface_target =
      is_numeric(adapter) ? adapter : cmd_quote_arg(adapter);
  timing.mark("resolve_adapter",
              "adapter=" + adapter + " if_index=" + if_index +
                  " internal_ip=" + internal_ip);

  const std::string default_gateway = get_default_gateway4();
  timing.mark("default_gateway",
              default_gateway.empty() ? "result=missing"
                                      : "gateway=" + default_gateway);
  if (!default_gateway.empty()) {
    for (const auto &ip : context.server_route_exceptions) {
      run_exit(ready_path,
               "route.exe delete " + ip + " mask 255.255.255.255");
      if (!run_with_retry(ready_path,
                          "route.exe add " + ip +
                              " mask 255.255.255.255 " + default_gateway,
                          2, 500, true)) {
        debug_log(ready_path, "failed to preserve server route " + ip);
      }
    }
  }
  timing.mark("preserve_server_routes",
              "count=" + std::to_string(context.server_route_exceptions.size()));

  bool ok = true;
  std::string adapter_mtu = effective_mtu(mtu, context.configured_mtu);
  if (!adapter_mtu.empty()) {
    if (adapter_mtu != mtu) {
      debug_log(ready_path, "ignoring low reported MTU=" + mtu +
                                ", using configured MTU=" + adapter_mtu);
    }
    ok = run_with_retry(
             ready_path,
             "netsh.exe interface ipv4 set subinterface " +
                 subinterface_target + " mtu=" + adapter_mtu + " store=active",
             3, 1000, false) &&
         ok;
  }
  timing.mark("set_mtu", adapter_mtu.empty() ? "result=skipped"
                                             : "mtu=" + adapter_mtu);

  ok = run_with_retry(ready_path,
                      "netsh.exe interface ipv4 set address " +
                          address_target + " static " + internal_ip + " " +
                          netmask,
                      5, 1000, false) &&
       ok;
  timing.mark("set_address", ok ? "result=ok" : "result=failed");

  Sleep(3000);
  timing.mark("wait_interface_registration", "sleep_ms=3000");

  for (const auto &cidr : context.custom_routes) {
    auto [network, mask] = cidr_to_network_and_mask(cidr);
    run_exit(ready_path, "route.exe delete " + network + " mask " + mask);
    std::string route_cmd = "route.exe add " + network + " mask " + mask +
                            " " + internal_ip;
    if (!if_index.empty())
      route_cmd += " if " + if_index;
    route_cmd += " metric 1";
    ok = run_with_retry(ready_path, route_cmd, 5, 1000, false) && ok;
  }
  timing.mark("add_split_routes",
              "count=" + std::to_string(context.custom_routes.size()) +
                  (ok ? " result=ok" : " result=failed"));

  if (!ok) {
    debug_log(ready_path, "native network configuration incomplete");
    timing.finish(false, "reason=network_configuration_incomplete");
    return false;
  }

  if (!write_ready_file(ready_path, adapter, internal_ip)) {
    debug_log(ready_path, "failed to write route-ready marker");
    timing.finish(false, "reason=write_route_ready_failed");
    return false;
  }

  debug_log(ready_path,
            "writeReadyFile tundev=" + adapter + " ip=" + internal_ip);
  timing.mark("write_route_ready", "interface=" + adapter);
  timing.finish(true, "interface=" + adapter + " internal_ip=" + internal_ip);
  return true;
}

std::pair<std::string, std::string>
cidr_to_network_and_mask(const std::string &cidr) {
  std::size_t slash = cidr.find('/');
  std::string network =
      (slash == std::string::npos) ? cidr : cidr.substr(0, slash);
  int prefix = 32;
  if (slash != std::string::npos) {
    try {
      prefix = std::stoi(cidr.substr(slash + 1));
    } catch (...) {
      prefix = 32;
    }
  }
  if (prefix < 0)
    prefix = 0;
  if (prefix > 32)
    prefix = 32;

  uint32_t mask_raw = prefix == 0 ? 0 : (~uint32_t(0) << (32 - prefix));
  in_addr addr{};
  addr.s_addr = htonl(mask_raw);
  char buf[INET_ADDRSTRLEN] = {0};
  inet_ntop(AF_INET, &addr, buf, sizeof(buf));
  return {network, std::string(buf)};
}
