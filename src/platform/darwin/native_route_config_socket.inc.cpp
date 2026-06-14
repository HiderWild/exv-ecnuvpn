int errno_or_io_error() { return errno == 0 ? EIO : errno; }

std::size_t aligned_sockaddr_length(const sockaddr *address) {
  if (!address)
    return 0;

  std::size_t length = address->sa_len;
  if (length == 0)
    length = sizeof(sockaddr);

  const std::size_t alignment = sizeof(long);
  return (length + alignment - 1) & ~(alignment - 1);
}

void append_sockaddr(std::vector<char> *message, const sockaddr *address) {
  if (!message || !address)
    return;

  const std::size_t length =
      address->sa_len == 0 ? sizeof(sockaddr) : address->sa_len;
  const std::size_t aligned_length = aligned_sockaddr_length(address);
  const std::size_t offset = message->size();
  message->resize(offset + aligned_length);
  std::memset(message->data() + offset, 0, aligned_length);
  std::memcpy(message->data() + offset, address, length);
}

sockaddr_in make_inet_sockaddr(std::uint32_t host_order) {
  sockaddr_in address{};
  address.sin_len = sizeof(sockaddr_in);
  address.sin_family = AF_INET;
  address.sin_addr.s_addr = htonl(host_order);
  return address;
}

sockaddr_dl make_link_sockaddr(std::uint32_t interface_index) {
  sockaddr_dl address{};
  address.sdl_len = sizeof(sockaddr_dl);
  address.sdl_family = AF_LINK;
  address.sdl_index = static_cast<unsigned short>(interface_index);
  return address;
}

std::string sockaddr_in_to_ipv4(const sockaddr_in *address) {
  if (!address)
    return {};
  return ipv4_from_host_order(ntohl(address->sin_addr.s_addr));
}

void copy_if_name(const sockaddr_dl *address, NativeDarwinUpstreamRoute *route) {
  if (!address || !route)
    return;

  if (address->sdl_index != 0)
    route->interface_index = static_cast<std::uint32_t>(address->sdl_index);
  if (address->sdl_nlen > 0)
    route->interface_name.assign(address->sdl_data, address->sdl_nlen);
}

void fill_interface_name_from_index(NativeDarwinUpstreamRoute *route) {
  if (!route || !route->interface_name.empty() || route->interface_index == 0)
    return;

  char name[IF_NAMESIZE] = {};
  if (if_indextoname(route->interface_index, name))
    route->interface_name = name;
}

int write_route_message(int route_socket, int type, int flags, int addrs,
                        const std::vector<const sockaddr *> &sockaddrs,
                        NativeDarwinUpstreamRoute *queried_route = nullptr,
                        std::uint32_t message_interface_index = 0) {
  static std::uint32_t sequence = 0;

  std::vector<char> message(sizeof(rt_msghdr));
  for (const sockaddr *address : sockaddrs)
    append_sockaddr(&message, address);

  rt_msghdr *header = reinterpret_cast<rt_msghdr *>(message.data());
  header->rtm_msglen = static_cast<unsigned short>(message.size());
  header->rtm_version = RTM_VERSION;
  header->rtm_type = type;
  header->rtm_flags = flags;
  header->rtm_addrs = addrs;
  header->rtm_seq = ++sequence;
  header->rtm_pid = getpid();
  if (message_interface_index != 0)
    header->rtm_index = static_cast<unsigned short>(message_interface_index);

  if (write(route_socket, message.data(), message.size()) < 0)
    return errno_or_io_error();

  if (!queried_route)
    return kNoError;

  char buffer[2048] = {};
  for (;;) {
    const ssize_t read_count = read(route_socket, buffer, sizeof(buffer));
    if (read_count < 0)
      return errno_or_io_error();
    if (read_count < static_cast<ssize_t>(sizeof(rt_msghdr)))
      continue;

    const rt_msghdr *reply = reinterpret_cast<const rt_msghdr *>(buffer);
    if (reply->rtm_seq != header->rtm_seq || reply->rtm_pid != header->rtm_pid)
      continue;
    if (reply->rtm_errno != 0)
      return reply->rtm_errno;

    const sockaddr *route_addrs[RTAX_MAX] = {};
    const char *cursor = buffer + sizeof(rt_msghdr);
    const char *end = buffer + read_count;
    for (int index = 0; index < RTAX_MAX; ++index) {
      if ((reply->rtm_addrs & (1 << index)) == 0)
        continue;
      if (cursor >= end)
        break;
      const sockaddr *address = reinterpret_cast<const sockaddr *>(cursor);
      route_addrs[index] = address;
      cursor += aligned_sockaddr_length(address);
    }

    if (route_addrs[RTAX_GATEWAY] &&
        route_addrs[RTAX_GATEWAY]->sa_family == AF_INET) {
      queried_route->gateway = sockaddr_in_to_ipv4(
          reinterpret_cast<const sockaddr_in *>(route_addrs[RTAX_GATEWAY]));
    } else if (route_addrs[RTAX_GATEWAY] &&
               route_addrs[RTAX_GATEWAY]->sa_family == AF_LINK) {
      copy_if_name(reinterpret_cast<const sockaddr_dl *>(
                       route_addrs[RTAX_GATEWAY]),
                   queried_route);
    }

    if (route_addrs[RTAX_IFP] &&
        route_addrs[RTAX_IFP]->sa_family == AF_LINK) {
      copy_if_name(reinterpret_cast<const sockaddr_dl *>(route_addrs[RTAX_IFP]),
                   queried_route);
    }
    fill_interface_name_from_index(queried_route);
    return kNoError;
  }
}

int open_route_socket() {
  const int route_socket = socket(PF_ROUTE, SOCK_RAW, AF_UNSPEC);
  return route_socket;
}

int set_interface_mtu_native(const std::string &interface_name, int mtu) {
  const int sock = socket(AF_INET, SOCK_DGRAM, 0);
  if (sock < 0)
    return errno_or_io_error();

  ifreq request{};
  std::snprintf(request.ifr_name, sizeof(request.ifr_name), "%s",
                interface_name.c_str());
  request.ifr_mtu = mtu;
  const int result = ioctl(sock, SIOCSIFMTU, &request);
  const int saved_errno = result == 0 ? kNoError : errno_or_io_error();
  close(sock);
  return saved_errno;
}

int get_best_route_native(const std::string &destination,
                          NativeDarwinUpstreamRoute *route) {
  if (!route)
    return EINVAL;

  std::uint32_t destination_host_order = 0;
  if (!parse_ipv4_host_order(destination, &destination_host_order))
    return EINVAL;

  const int route_socket = open_route_socket();
  if (route_socket < 0)
    return errno_or_io_error();

  sockaddr_in destination_address = make_inet_sockaddr(destination_host_order);
  const int result = write_route_message(
      route_socket, RTM_GET, RTF_UP | RTF_HOST, RTA_DST,
      {reinterpret_cast<const sockaddr *>(&destination_address)}, route);
  close(route_socket);
  return result;
}

std::uint32_t route_interface_index(const NativeDarwinRoute &route) {
  if (route.interface_index != 0)
    return route.interface_index;
  if (route.interface_name.empty())
    return 0;
  return if_nametoindex(route.interface_name.c_str());
}

std::uint32_t route_message_interface_index(const NativeDarwinRoute &route) {
  if (!route.message_interface_scoped)
    return 0;
  if (route.message_interface_index != 0)
    return route.message_interface_index;
  return route_interface_index(route);
}

int update_route_native(int message_type, const NativeDarwinRoute &route) {
  std::uint32_t destination_host_order = 0;
  std::uint32_t netmask_host_order = 0;
  if (!parse_ipv4_host_order(route.destination, &destination_host_order) ||
      !parse_ipv4_host_order(route.netmask, &netmask_host_order))
    return EINVAL;

  sockaddr_in destination = make_inet_sockaddr(destination_host_order);
  sockaddr_in netmask = make_inet_sockaddr(netmask_host_order);
  sockaddr_in gateway_inet{};
  sockaddr_dl gateway_link{};
  const sockaddr *gateway = nullptr;

  int flags = RTF_UP | RTF_STATIC;
  if (route.prefix_length == 32)
    flags |= RTF_HOST;

  const std::uint32_t message_interface_index =
      route_message_interface_index(route);
  if (route.message_interface_scoped) {
    if (message_interface_index == 0)
      return ENXIO;
#if defined(RTF_IFSCOPE)
    flags |= RTF_IFSCOPE;
#else
    return ENOTSUP;
#endif
  }

  if (!route.gateway.empty()) {
    std::uint32_t gateway_host_order = 0;
    if (!parse_ipv4_host_order(route.gateway, &gateway_host_order))
      return EINVAL;
    gateway_inet = make_inet_sockaddr(gateway_host_order);
    gateway = reinterpret_cast<const sockaddr *>(&gateway_inet);
    flags |= RTF_GATEWAY;
  } else {
    const std::uint32_t interface_index = route_interface_index(route);
    if (interface_index == 0)
      return ENXIO;
    gateway_link = make_link_sockaddr(interface_index);
    gateway = reinterpret_cast<const sockaddr *>(&gateway_link);
  }

  const int route_socket = open_route_socket();
  if (route_socket < 0)
    return errno_or_io_error();

  const int result = write_route_message(
      route_socket, message_type, flags, RTA_DST | RTA_GATEWAY | RTA_NETMASK,
      {reinterpret_cast<const sockaddr *>(&destination), gateway,
       reinterpret_cast<const sockaddr *>(&netmask)},
      nullptr, message_interface_index);
  close(route_socket);
  return result;
}
