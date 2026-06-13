const char *
native_darwin_route_config_error_code(NativeDarwinRouteConfigError error) {
  switch (error) {
  case NativeDarwinRouteConfigError::none:
    return "none";
  case NativeDarwinRouteConfigError::api_missing:
    return "api_missing";
  case NativeDarwinRouteConfigError::invalid_interface:
    return "invalid_interface";
  case NativeDarwinRouteConfigError::invalid_mtu:
    return "invalid_mtu";
  case NativeDarwinRouteConfigError::invalid_route:
    return "invalid_route";
  case NativeDarwinRouteConfigError::mtu_set_failed:
    return "mtu_set_failed";
  case NativeDarwinRouteConfigError::upstream_route_failed:
    return "upstream_route_failed";
  case NativeDarwinRouteConfigError::route_add_failed:
    return "route_add_failed";
  case NativeDarwinRouteConfigError::route_delete_failed:
    return "route_delete_failed";
  }
  return "unknown";
}

NativeDarwinRouteApi default_native_darwin_route_api() {
  NativeDarwinRouteApi api;
#if defined(__APPLE__)
  api.set_interface_mtu =
      [](const std::string &interface_name, int mtu) {
        return set_interface_mtu_native(interface_name, mtu);
      };
  api.get_best_route =
      [](const std::string &destination, NativeDarwinUpstreamRoute &route) {
        return get_best_route_native(destination, &route);
      };
  api.add_route = [](const NativeDarwinRoute &route) {
    return update_route_native(RTM_ADD, route);
  };
  api.delete_route = [](const NativeDarwinRoute &route) {
    return update_route_native(RTM_DELETE, route);
  };
  api.interface_index_from_name = [](const std::string &interface_name) {
    return if_nametoindex(interface_name.c_str());
  };
#else
  api.set_interface_mtu = [](const std::string &, int) {
    return unsupported_native_api_error();
  };
  api.get_best_route = [](const std::string &, NativeDarwinUpstreamRoute &) {
    return unsupported_native_api_error();
  };
  api.add_route = [](const NativeDarwinRoute &) {
    return unsupported_native_api_error();
  };
  api.delete_route = [](const NativeDarwinRoute &) {
    return unsupported_native_api_error();
  };
  api.interface_index_from_name = [](const std::string &) {
    return static_cast<std::uint32_t>(0);
  };
#endif
  return api;
}

NativeDarwinRouteConfig::NativeDarwinRouteConfig(
    NativeDarwinRouteApi api, NativeDarwinRouteConfigOptions options)
    : api_(std::move(api)), options_(std::move(options)) {}

NativeDarwinRouteConfigResult NativeDarwinRouteConfig::configure(
    const vpn_engine::TunnelMetadata &metadata) {
  if (!has_required_api(api_))
    return failure(NativeDarwinRouteConfigError::api_missing,
                   "native Darwin route API table is incomplete");

  const std::string interface_name =
      effective_interface_name(options_, metadata);
  std::uint32_t interface_index =
      effective_interface_index(options_, metadata);
  if (interface_name.empty())
    return failure(NativeDarwinRouteConfigError::invalid_interface,
                   "missing utun interface name");

  const int mtu = choose_effective_mtu(metadata.mtu, options_.configured_mtu);
  if (mtu == 0)
    return failure(NativeDarwinRouteConfigError::invalid_mtu,
                   "no usable tunnel or configured MTU");

  NativeDarwinRouteApi::ErrorCode error =
      api_.set_interface_mtu(interface_name, mtu);
  if (error != kNoError)
    return failure(NativeDarwinRouteConfigError::mtu_set_failed,
                   "setting utun interface MTU failed", interface_name, error);
  effective_mtu_ = mtu;

  std::vector<NativeDarwinRoute> routes;
  std::string invalid_target;
  if (!plan_routes(metadata, &routes, &invalid_target))
    return failure(NativeDarwinRouteConfigError::invalid_route,
                   "invalid IPv4 route in tunnel metadata", invalid_target);

  for (NativeDarwinRoute &route : routes) {
    if (route.server_bypass) {
      NativeDarwinUpstreamRoute upstream_route;
      error = api_.get_best_route(route.destination, upstream_route);
      if (error != kNoError)
        return failure(NativeDarwinRouteConfigError::upstream_route_failed,
                       "querying upstream route failed", route.cidr, error);
      route.interface_name = upstream_route.interface_name;
      route.interface_index = upstream_route.interface_index;
      route.gateway = upstream_route.gateway;
      scope_route_message_to_interface(&route);
    } else {
      interface_index =
          stable_interface_index(api_, interface_name, interface_index);
      route.interface_name = interface_name;
      route.interface_index = interface_index;
      route.gateway.clear();
      scope_route_message_to_interface(&route);
    }

    error = api_.add_route(route);
    if (error != kNoError)
      return failure(NativeDarwinRouteConfigError::route_add_failed,
                     "adding Darwin route failed", route.cidr, error);

    owned_routes_.push_back(route);
  }

  NativeDarwinRouteConfigResult result;
  result.effective_mtu = effective_mtu_;
  return result;
}

NativeDarwinRouteConfigResult NativeDarwinRouteConfig::cleanup() {
  if (owned_routes_.empty())
    return {};

  if (!api_.delete_route)
    return failure(NativeDarwinRouteConfigError::api_missing,
                   "native Darwin route delete API is missing");

  while (!owned_routes_.empty()) {
    NativeDarwinRoute route = owned_routes_.back();
    NativeDarwinRouteApi::ErrorCode error = api_.delete_route(route);
    if (error != kNoError)
      return failure(NativeDarwinRouteConfigError::route_delete_failed,
                     "deleting Darwin route failed", route.cidr, error);
    owned_routes_.pop_back();
  }

  return {};
}

const std::vector<NativeDarwinRoute> &
NativeDarwinRouteConfig::owned_routes() const {
  return owned_routes_;
}

int NativeDarwinRouteConfig::effective_mtu() const { return effective_mtu_; }
