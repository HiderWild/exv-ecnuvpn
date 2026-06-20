#include "platform/win32/native_ip_config.hpp"

#ifndef WIN32_LEAN_AND_MEAN
#define WIN32_LEAN_AND_MEAN
#endif
#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>
#include <iphlpapi.h>
#include <netioapi.h>

#include <algorithm>
#include <cctype>
#include <cwctype>
#include <limits>
#include <set>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

namespace ecnuvpn {
namespace platform {
namespace {

constexpr std::uint32_t kNoError = 0;
constexpr int kMinimumUsableMtu = 1200;
constexpr int kMaximumMtu = 1500;
constexpr ULONG kDnsInterfaceSettingsVersion1 = 1;
constexpr ULONG64 kDnsSettingNameServer = 0x0002;
constexpr ULONG64 kDnsSettingSearchList = 0x0004;
constexpr ULONG64 kDnsSettingDomain = 0x0020;

struct EcnuDnsInterfaceSettings {
  ULONG Version;
  ULONG64 Flags;
  PWSTR Domain;
  PWSTR NameServer;
  PWSTR SearchList;
  ULONG RegistrationEnabled;
  ULONG RegisterAdapterName;
  ULONG EnableLLMNR;
  ULONG QueryAdapterName;
  PWSTR ProfileNameServer;
};

using GetInterfaceDnsSettingsFn =
    NETIO_STATUS(WINAPI *)(GUID, EcnuDnsInterfaceSettings *);
using SetInterfaceDnsSettingsFn =
    NETIO_STATUS(WINAPI *)(GUID, const EcnuDnsInterfaceSettings *);
using FreeInterfaceDnsSettingsFn = VOID(WINAPI *)(EcnuDnsInterfaceSettings *);

NativeIpConfigResult failure(NativeIpConfigError error,
                             std::string message = {},
                             std::string target = {},
                             std::uint32_t system_error = 0) {
  NativeIpConfigResult result;
  result.error = error;
  result.message = std::move(message);
  result.target = std::move(target);
  result.system_error = system_error;
  return result;
}

bool has_required_api(const NativeIpHelperApi &api) {
  return static_cast<bool>(api.initialize_unicast_ip_address_entry) &&
         static_cast<bool>(api.create_unicast_ip_address_entry) &&
         static_cast<bool>(api.set_interface_mtu) &&
         static_cast<bool>(api.get_best_route2) &&
         static_cast<bool>(api.create_ip_forward_entry2) &&
         static_cast<bool>(api.delete_ip_forward_entry2);
}

HMODULE iphlpapi_module() {
  HMODULE module = GetModuleHandleW(L"Iphlpapi.dll");
  if (module)
    return module;
  return LoadLibraryW(L"Iphlpapi.dll");
}

template <typename Fn> Fn load_iphlpapi_proc(const char *name) {
  HMODULE module = iphlpapi_module();
  if (!module)
    return nullptr;
  return reinterpret_cast<Fn>(GetProcAddress(module, name));
}

NativeIpHelperApi::ErrorCode interface_guid_from_index(
    std::uint32_t interface_index, GUID *guid) {
  if (interface_index == 0 || !guid)
    return ERROR_INVALID_PARAMETER;

  NET_LUID luid{};
  NETIO_STATUS status = ConvertInterfaceIndexToLuid(
      static_cast<NET_IFINDEX>(interface_index), &luid);
  if (status != NO_ERROR)
    return static_cast<NativeIpHelperApi::ErrorCode>(status);

  status = ConvertInterfaceLuidToGuid(&luid, guid);
  if (status != NO_ERROR)
    return static_cast<NativeIpHelperApi::ErrorCode>(status);

  return static_cast<NativeIpHelperApi::ErrorCode>(NO_ERROR);
}

std::wstring utf8_to_wide(const std::string &value) {
  if (value.empty())
    return {};

  const int required = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS,
                                           value.c_str(), -1, nullptr, 0);
  if (required <= 0)
    return {};

  std::wstring wide(static_cast<std::size_t>(required), L'\0');
  if (required > 0) {
    MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, value.c_str(), -1,
                        wide.data(), required);
  }
  if (!wide.empty() && wide.back() == L'\0')
    wide.pop_back();
  return wide;
}

std::string wide_to_utf8(PCWSTR value) {
  if (!value || value[0] == L'\0')
    return {};

  const int required =
      WideCharToMultiByte(CP_UTF8, 0, value, -1, nullptr, 0, nullptr, nullptr);
  if (required <= 0)
    return {};

  std::string out(static_cast<std::size_t>(required), '\0');
  if (required > 0) {
    WideCharToMultiByte(CP_UTF8, 0, value, -1, out.data(), required, nullptr,
                        nullptr);
  }
  if (!out.empty() && out.back() == '\0')
    out.pop_back();
  return out;
}

std::wstring join_wide_list(const std::vector<std::string> &values) {
  std::wstring joined;
  for (const std::string &value : values) {
    if (!joined.empty())
      joined.push_back(L' ');
    joined += utf8_to_wide(value);
  }
  return joined;
}

std::vector<std::string> split_wide_dns_list(PCWSTR value) {
  std::vector<std::string> out;
  if (!value)
    return out;

  std::wstring current;
  for (const wchar_t *cursor = value;; ++cursor) {
    const wchar_t ch = *cursor;
    if (ch == L'\0' || ch == L',' || std::iswspace(ch)) {
      if (!current.empty()) {
        out.push_back(wide_to_utf8(current.c_str()));
        current.clear();
      }
      if (ch == L'\0')
        break;
      continue;
    }
    current.push_back(ch);
  }
  return out;
}

NativeIpHelperApi::ErrorCode get_interface_dns_settings_native(
    std::uint32_t interface_index, NativeDnsSettings &settings) {
  auto get_dns =
      load_iphlpapi_proc<GetInterfaceDnsSettingsFn>("GetInterfaceDnsSettings");
  auto free_dns =
      load_iphlpapi_proc<FreeInterfaceDnsSettingsFn>("FreeInterfaceDnsSettings");
  if (!get_dns || !free_dns)
    return ERROR_PROC_NOT_FOUND;

  GUID guid{};
  NativeIpHelperApi::ErrorCode error =
      interface_guid_from_index(interface_index, &guid);
  if (error != NO_ERROR)
    return error;

  EcnuDnsInterfaceSettings native{};
  native.Version = kDnsInterfaceSettingsVersion1;
  NETIO_STATUS status = get_dns(guid, &native);
  if (status != NO_ERROR)
    return static_cast<NativeIpHelperApi::ErrorCode>(status);

  settings.servers = split_wide_dns_list(native.NameServer);
  settings.search_domain = wide_to_utf8(native.Domain);
  settings.suffixes = split_wide_dns_list(native.SearchList);
  free_dns(&native);
  return static_cast<NativeIpHelperApi::ErrorCode>(NO_ERROR);
}

NativeIpHelperApi::ErrorCode set_interface_dns_settings_native(
    std::uint32_t interface_index, const NativeDnsSettings &settings) {
  auto set_dns =
      load_iphlpapi_proc<SetInterfaceDnsSettingsFn>("SetInterfaceDnsSettings");
  if (!set_dns)
    return ERROR_PROC_NOT_FOUND;

  GUID guid{};
  NativeIpHelperApi::ErrorCode error =
      interface_guid_from_index(interface_index, &guid);
  if (error != NO_ERROR)
    return error;

  std::wstring name_server = join_wide_list(settings.servers);
  std::wstring domain = utf8_to_wide(settings.search_domain);
  std::wstring search_list = join_wide_list(settings.suffixes);

  EcnuDnsInterfaceSettings native{};
  native.Version = kDnsInterfaceSettingsVersion1;
  native.Flags =
      kDnsSettingNameServer | kDnsSettingSearchList | kDnsSettingDomain;
  native.NameServer = name_server.empty() ? const_cast<PWSTR>(L"")
                                          : name_server.data();
  native.Domain = domain.empty() ? const_cast<PWSTR>(L"") : domain.data();
  native.SearchList =
      search_list.empty() ? const_cast<PWSTR>(L"") : search_list.data();

  NETIO_STATUS status = set_dns(guid, &native);
  return static_cast<NativeIpHelperApi::ErrorCode>(status);
}

std::string trim_ascii(const std::string &value) {
  std::size_t first = 0;
  while (first < value.size() &&
         std::isspace(static_cast<unsigned char>(value[first])))
    ++first;

  std::size_t last = value.size();
  while (last > first &&
         std::isspace(static_cast<unsigned char>(value[last - 1])))
    --last;

  return value.substr(first, last - first);
}

bool parse_ipv4_host_order(const std::string &value, std::uint32_t *out) {
  if (!out)
    return false;

  IN_ADDR address{};
  if (InetPtonA(AF_INET, value.c_str(), &address) != 1)
    return false;

  *out = ntohl(address.S_un.S_addr);
  return true;
}

std::string ipv4_from_host_order(std::uint32_t value) {
  IN_ADDR address{};
  address.S_un.S_addr = htonl(value);
  char buffer[INET_ADDRSTRLEN] = {0};
  if (!InetNtopA(AF_INET, &address, buffer, sizeof(buffer)))
    return {};
  return std::string(buffer);
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

bool prefix_from_netmask(const std::string &netmask, int *prefix) {
  std::uint32_t mask = 0;
  if (!parse_ipv4_host_order(netmask, &mask) || !prefix)
    return false;

  int bits = 0;
  bool saw_zero = false;
  for (int bit = 31; bit >= 0; --bit) {
    const bool one = (mask & (std::uint32_t{1} << bit)) != 0;
    if (one) {
      if (saw_zero)
        return false;
      ++bits;
    } else {
      saw_zero = true;
    }
  }

  *prefix = bits;
  return true;
}

bool normalize_cidr(const std::string &input, int default_prefix,
                    NativeIpRoute *route) {
  if (!route)
    return false;

  const std::string value = trim_ascii(input);
  if (value.empty())
    return false;

  const std::size_t slash = value.find('/');
  if (slash != std::string::npos && value.find('/', slash + 1) != std::string::npos)
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

  const std::uint32_t mask =
      prefix == 0 ? std::uint32_t{0}
                  : (std::numeric_limits<std::uint32_t>::max()
                     << (32 - prefix));
  const std::uint32_t network = host_order & mask;
  route->destination = ipv4_from_host_order(network);
  route->prefix_length = prefix;
  route->cidr = route->destination + "/" + std::to_string(prefix);
  return !route->destination.empty();
}

int choose_effective_mtu(int tunnel_mtu, int configured_mtu) {
  if (tunnel_mtu >= kMinimumUsableMtu && tunnel_mtu <= kMaximumMtu)
    return tunnel_mtu;
  if (configured_mtu >= kMinimumUsableMtu && configured_mtu <= kMaximumMtu)
    return configured_mtu;
  return 0;
}

std::uint32_t effective_interface_index(
    const NativeIpConfigOptions &options,
    const vpn_engine::TunnelMetadata &metadata) {
  if (options.interface_index != 0)
    return options.interface_index;
  if (metadata.interface_index > 0)
    return static_cast<std::uint32_t>(metadata.interface_index);
  return 0;
}

bool plan_routes(const vpn_engine::TunnelMetadata &metadata,
                 std::vector<NativeIpRoute> *routes) {
  if (!routes)
    return false;
  routes->clear();

  std::set<std::string> seen;

  auto append_route = [&](const std::string &cidr, bool server_bypass,
                          int default_prefix) {
    NativeIpRoute route;
    if (!normalize_cidr(cidr, default_prefix, &route))
      return false;
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

bool make_sockaddr_ipv4(const std::string &ip, SOCKADDR_INET *out) {
  if (!out)
    return false;
  *out = SOCKADDR_INET{};
  out->si_family = AF_INET;
  out->Ipv4.sin_family = AF_INET;
  return InetPtonA(AF_INET, ip.c_str(), &out->Ipv4.sin_addr) == 1;
}

std::string sockaddr_ipv4_to_string(const SOCKADDR_INET &address) {
  if (address.si_family != AF_INET)
    return {};

  char buffer[INET_ADDRSTRLEN] = {0};
  if (!InetNtopA(AF_INET, const_cast<IN_ADDR *>(&address.Ipv4.sin_addr),
                 buffer, sizeof(buffer)))
    return {};
  return std::string(buffer);
}

MIB_UNICASTIPADDRESS_ROW
to_unicast_row(const NativeUnicastAddress &address) {
  MIB_UNICASTIPADDRESS_ROW row;
  InitializeUnicastIpAddressEntry(&row);
  row.InterfaceIndex = address.interface_index;
  row.OnLinkPrefixLength = static_cast<UINT8>(address.prefix_length);
  row.PrefixOrigin = IpPrefixOriginManual;
  row.SuffixOrigin = IpSuffixOriginManual;
  row.ValidLifetime = address.valid_lifetime;
  row.PreferredLifetime = address.preferred_lifetime;
  row.SkipAsSource = address.skip_as_source ? TRUE : FALSE;
  if (address.dad_state_preferred)
    row.DadState = IpDadStatePreferred;
  make_sockaddr_ipv4(address.address, &row.Address);
  return row;
}

MIB_IPFORWARD_ROW2 to_route_row(const NativeIpRoute &route) {
  MIB_IPFORWARD_ROW2 row;
  InitializeIpForwardEntry(&row);
  row.InterfaceIndex = route.interface_index;
  row.DestinationPrefix.PrefixLength =
      static_cast<UINT8>(route.prefix_length);
  make_sockaddr_ipv4(route.destination, &row.DestinationPrefix.Prefix);
  make_sockaddr_ipv4(route.next_hop.empty() ? "0.0.0.0" : route.next_hop,
                     &row.NextHop);
  row.Metric = 1;
  row.Protocol = static_cast<NL_ROUTE_PROTOCOL>(MIB_IPPROTO_NETMGMT);
  return row;
}

} // namespace

const char *native_ip_config_error_code(NativeIpConfigError error) {
  switch (error) {
  case NativeIpConfigError::none:
    return "none";
  case NativeIpConfigError::api_missing:
    return "api_missing";
  case NativeIpConfigError::invalid_interface:
    return "invalid_interface";
  case NativeIpConfigError::invalid_address:
    return "invalid_address";
  case NativeIpConfigError::invalid_mtu:
    return "invalid_mtu";
  case NativeIpConfigError::invalid_route:
    return "invalid_route";
  case NativeIpConfigError::address_create_failed:
    return "address_create_failed";
  case NativeIpConfigError::mtu_set_failed:
    return "mtu_set_failed";
  case NativeIpConfigError::best_route_failed:
    return "best_route_failed";
  case NativeIpConfigError::route_create_failed:
    return "route_create_failed";
  case NativeIpConfigError::route_delete_failed:
    return "route_delete_failed";
  }
  return "unknown";
}

NativeIpHelperApi default_native_ip_helper_api() {
  NativeIpHelperApi api;
  api.initialize_unicast_ip_address_entry = [](NativeUnicastAddress &) {};
  api.create_unicast_ip_address_entry =
      [](const NativeUnicastAddress &address) {
        MIB_UNICASTIPADDRESS_ROW row = to_unicast_row(address);
        return static_cast<NativeIpHelperApi::ErrorCode>(
            CreateUnicastIpAddressEntry(&row));
      };
  api.set_interface_mtu = [](std::uint32_t interface_index, int mtu) {
    MIB_IPINTERFACE_ROW row;
    InitializeIpInterfaceEntry(&row);
    row.Family = AF_INET;
    row.InterfaceIndex = interface_index;
    NETIO_STATUS status = GetIpInterfaceEntry(&row);
    if (status != NO_ERROR)
      return static_cast<NativeIpHelperApi::ErrorCode>(status);
    row.NlMtu = static_cast<ULONG>(mtu);
    return static_cast<NativeIpHelperApi::ErrorCode>(
        SetIpInterfaceEntry(&row));
  };
  api.get_best_route2 =
      [](const std::string &destination, NativeBestRoute &route) {
        SOCKADDR_INET destination_address{};
        if (!make_sockaddr_ipv4(destination, &destination_address))
          return static_cast<NativeIpHelperApi::ErrorCode>(
              ERROR_INVALID_PARAMETER);

        MIB_IPFORWARD_ROW2 best_route{};
        SOCKADDR_INET best_source{};
        NETIO_STATUS status =
            GetBestRoute2(nullptr, 0, nullptr, &destination_address, 0,
                          &best_route, &best_source);
        if (status != NO_ERROR)
          return static_cast<NativeIpHelperApi::ErrorCode>(status);

        route.interface_index =
            static_cast<std::uint32_t>(best_route.InterfaceIndex);
        route.next_hop = sockaddr_ipv4_to_string(best_route.NextHop);
        return static_cast<NativeIpHelperApi::ErrorCode>(NO_ERROR);
      };
  api.create_ip_forward_entry2 = [](const NativeIpRoute &route) {
    MIB_IPFORWARD_ROW2 row = to_route_row(route);
    return static_cast<NativeIpHelperApi::ErrorCode>(
        CreateIpForwardEntry2(&row));
  };
  api.delete_ip_forward_entry2 = [](const NativeIpRoute &route) {
    MIB_IPFORWARD_ROW2 row = to_route_row(route);
    return static_cast<NativeIpHelperApi::ErrorCode>(
        DeleteIpForwardEntry2(&row));
  };
  api.get_interface_dns_settings = get_interface_dns_settings_native;
  api.set_interface_dns_settings = set_interface_dns_settings_native;
  return api;
}

NativeIpConfig::NativeIpConfig(NativeIpHelperApi api,
                               NativeIpConfigOptions options)
    : api_(std::move(api)), options_(options) {}

NativeIpConfigResult
NativeIpConfig::configure(const vpn_engine::TunnelMetadata &metadata) {
  if (!has_required_api(api_))
    return failure(NativeIpConfigError::api_missing,
                   "native IP Helper API table is incomplete");

  const std::uint32_t interface_index =
      effective_interface_index(options_, metadata);
  if (interface_index == 0)
    return failure(NativeIpConfigError::invalid_interface,
                   "missing tunnel interface index");

  int prefix_length = 0;
  if (metadata.internal_ip4_address.empty() ||
      !prefix_from_netmask(metadata.internal_ip4_netmask, &prefix_length))
    return failure(NativeIpConfigError::invalid_address,
                   "invalid tunnel IPv4 address or netmask",
                   metadata.internal_ip4_address);

  std::uint32_t ignored = 0;
  if (!parse_ipv4_host_order(metadata.internal_ip4_address, &ignored))
    return failure(NativeIpConfigError::invalid_address,
                   "invalid tunnel IPv4 address",
                   metadata.internal_ip4_address);

  const int mtu = choose_effective_mtu(metadata.mtu, options_.configured_mtu);
  if (mtu == 0)
    return failure(NativeIpConfigError::invalid_mtu,
                   "no usable tunnel or configured MTU");

  NativeIpHelperApi::ErrorCode error = kNoError;
  if (options_.configure_address) {
    NativeUnicastAddress address;
    address.interface_index = interface_index;
    address.address = metadata.internal_ip4_address;
    address.prefix_length = prefix_length;
    api_.initialize_unicast_ip_address_entry(address);

    error = api_.create_unicast_ip_address_entry(address);
    if (error != kNoError && error != ERROR_OBJECT_ALREADY_EXISTS &&
        error != ERROR_ALREADY_EXISTS)
      return failure(NativeIpConfigError::address_create_failed,
                     "CreateUnicastIpAddressEntry failed", address.address,
                     error);
  }

  error = api_.set_interface_mtu(interface_index, mtu);
  if (error != kNoError && error != ERROR_INVALID_PARAMETER)
    return failure(NativeIpConfigError::mtu_set_failed,
                   "setting tunnel interface MTU failed", std::to_string(mtu),
                   error);
  effective_mtu_ = mtu;

  std::vector<NativeIpRoute> routes;
  if (!plan_routes(metadata, &routes))
    return failure(NativeIpConfigError::invalid_route,
                   "invalid IPv4 route in tunnel metadata");

  for (NativeIpRoute &route : routes) {
    if (route.server_bypass) {
      NativeBestRoute best_route;
      error = api_.get_best_route2(route.destination, best_route);
      if (error != kNoError)
        return failure(NativeIpConfigError::best_route_failed,
                       "GetBestRoute2 failed", route.cidr, error);
      route.interface_index = best_route.interface_index;
      route.next_hop = best_route.next_hop;
    } else {
      route.interface_index = interface_index;
      // Wintun routes are directly attached to the tunnel interface. Using the
      // local tunnel address as a gateway leaves the route out of Windows'
      // active route selection, especially when another TUN owns the default
      // route.
      route.next_hop.clear();
    }

    error = api_.create_ip_forward_entry2(route);
    if (error != kNoError)
      return failure(NativeIpConfigError::route_create_failed,
                     "CreateIpForwardEntry2 failed", route.cidr, error);

    owned_routes_.push_back(route);
  }

  NativeIpConfigResult result;
  result.effective_mtu = effective_mtu_;
  return result;
}

NativeIpConfigResult NativeIpConfig::cleanup() {
  if (owned_routes_.empty())
    return {};

  if (!api_.delete_ip_forward_entry2)
    return failure(NativeIpConfigError::api_missing,
                   "native IP Helper delete route API is missing");

  while (!owned_routes_.empty()) {
    NativeIpRoute route = owned_routes_.back();
    NativeIpHelperApi::ErrorCode error = api_.delete_ip_forward_entry2(route);
    if (error != kNoError && error != ERROR_NOT_FOUND)
      return failure(NativeIpConfigError::route_delete_failed,
                     "DeleteIpForwardEntry2 failed", route.cidr, error);
    owned_routes_.pop_back();
  }

  return {};
}

const std::vector<NativeIpRoute> &NativeIpConfig::owned_routes() const {
  return owned_routes_;
}

int NativeIpConfig::effective_mtu() const { return effective_mtu_; }

} // namespace platform
} // namespace ecnuvpn
