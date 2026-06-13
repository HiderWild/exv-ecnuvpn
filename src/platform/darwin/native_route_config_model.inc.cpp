NativeDarwinRouteConfigResult
failure(NativeDarwinRouteConfigError error, std::string message = {},
        std::string target = {}, int system_error = 0) {
  NativeDarwinRouteConfigResult result;
  result.error = error;
  result.message = std::move(message);
  result.target = std::move(target);
  result.system_error = system_error;
  return result;
}

bool has_required_api(const NativeDarwinRouteApi &api) {
  return static_cast<bool>(api.set_interface_mtu) &&
         static_cast<bool>(api.get_best_route) &&
         static_cast<bool>(api.add_route) &&
         static_cast<bool>(api.delete_route);
}

std::string trim_ascii(const std::string &value) {
  std::size_t first = 0;
  while (first < value.size() &&
         (value[first] == ' ' || value[first] == '\t' ||
          value[first] == '\r' || value[first] == '\n'))
    ++first;

  std::size_t last = value.size();
  while (last > first &&
         (value[last - 1] == ' ' || value[last - 1] == '\t' ||
          value[last - 1] == '\r' || value[last - 1] == '\n'))
    --last;

  return value.substr(first, last - first);
}

bool parse_ipv4_host_order(const std::string &value, std::uint32_t *out) {
  if (!out)
    return false;

  std::uint32_t octets[4] = {};
  std::size_t offset = 0;
  for (int i = 0; i < 4; ++i) {
    if (offset >= value.size() || value[offset] < '0' ||
        value[offset] > '9')
      return false;

    int octet = 0;
    int digits = 0;
    while (offset < value.size() && value[offset] >= '0' &&
           value[offset] <= '9') {
      octet = octet * 10 + (value[offset] - '0');
      if (octet > 255)
        return false;
      ++digits;
      ++offset;
    }

    if (digits == 0)
      return false;

    octets[i] = static_cast<std::uint32_t>(octet);
    if (i < 3) {
      if (offset >= value.size() || value[offset] != '.')
        return false;
      ++offset;
    }
  }

  if (offset != value.size())
    return false;

  *out = (octets[0] << 24) | (octets[1] << 16) | (octets[2] << 8) |
         octets[3];
  return true;
}

std::string ipv4_from_host_order(std::uint32_t value) {
  std::ostringstream out;
  out << ((value >> 24) & 0xff) << '.' << ((value >> 16) & 0xff) << '.'
      << ((value >> 8) & 0xff) << '.' << (value & 0xff);
  return out.str();
}

bool parse_prefix(const std::string &value, int *out) {
  if (value.empty() || !out)
    return false;

  int prefix = 0;
  for (char ch : value) {
    if (ch < '0' || ch > '9')
      return false;
    prefix = prefix * 10 + (ch - '0');
    if (prefix > 32)
      return false;
  }

  *out = prefix;
  return true;
}

std::uint32_t mask_from_prefix(int prefix) {
  if (prefix <= 0)
    return 0;
  if (prefix >= 32)
    return std::numeric_limits<std::uint32_t>::max();
  return std::numeric_limits<std::uint32_t>::max() << (32 - prefix);
}

bool normalize_cidr(const std::string &input, int default_prefix,
                    NativeDarwinRoute *route) {
  if (!route)
    return false;

  const std::string value = trim_ascii(input);
  if (value.empty())
    return false;

  const std::size_t slash = value.find('/');
  if (slash != std::string::npos &&
      value.find('/', slash + 1) != std::string::npos)
    return false;

  const std::string address =
      slash == std::string::npos ? value : value.substr(0, slash);
  int prefix = default_prefix;
  if (slash != std::string::npos &&
      !parse_prefix(value.substr(slash + 1), &prefix))
    return false;

  std::uint32_t host_order = 0;
  if (!parse_ipv4_host_order(address, &host_order))
    return false;

  const std::uint32_t mask = mask_from_prefix(prefix);
  const std::uint32_t network = host_order & mask;
  route->destination = ipv4_from_host_order(network);
  route->netmask = ipv4_from_host_order(mask);
  route->prefix_length = prefix;
  route->cidr = route->destination + "/" + std::to_string(prefix);
  return true;
}

int choose_effective_mtu(int tunnel_mtu, int configured_mtu) {
  if (tunnel_mtu >= kMinimumUsableMtu && tunnel_mtu <= kMaximumMtu)
    return tunnel_mtu;
  if (configured_mtu >= kMinimumUsableMtu && configured_mtu <= kMaximumMtu)
    return configured_mtu;
  return 0;
}

std::string effective_interface_name(
    const NativeDarwinRouteConfigOptions &options,
    const vpn_engine::TunnelMetadata &metadata) {
  if (!options.interface_name.empty())
    return options.interface_name;
  return metadata.interface_name;
}

std::uint32_t effective_interface_index(
    const NativeDarwinRouteConfigOptions &options,
    const vpn_engine::TunnelMetadata &metadata) {
  if (options.interface_index != 0)
    return options.interface_index;
  if (metadata.interface_index > 0)
    return static_cast<std::uint32_t>(metadata.interface_index);
  return 0;
}

std::uint32_t stable_interface_index(const NativeDarwinRouteApi &api,
                                     const std::string &interface_name,
                                     std::uint32_t known_index) {
  if (known_index != 0)
    return known_index;
  if (interface_name.empty() || !api.interface_index_from_name)
    return 0;
  return api.interface_index_from_name(interface_name);
}

void scope_route_message_to_interface(NativeDarwinRoute *route) {
  if (!route)
    return;

  route->message_interface_scoped = true;
  route->message_interface_index = route->interface_index;
}

bool plan_routes(const vpn_engine::TunnelMetadata &metadata,
                 std::vector<NativeDarwinRoute> *routes,
                 std::string *invalid_target) {
  if (!routes)
    return false;
  routes->clear();

  std::set<std::string> seen;

  auto append_route = [&](const std::string &cidr, bool server_bypass,
                          int default_prefix) {
    NativeDarwinRoute route;
    if (!normalize_cidr(cidr, default_prefix, &route)) {
      if (invalid_target)
        *invalid_target = cidr;
      return false;
    }
    route.server_bypass = server_bypass;
    if (!seen.insert(route.cidr).second)
      return true;
    routes->push_back(std::move(route));
    return true;
  };

  for (const std::string &bypass : metadata.server_bypass_ips) {
    if (!append_route(bypass, true, 32))
      return false;
  }
  for (const std::string &split : metadata.routes) {
    if (!append_route(split, false, 32))
      return false;
  }

  return true;
}

#if defined(__APPLE__)
