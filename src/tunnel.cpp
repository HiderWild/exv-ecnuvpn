#include "tunnel.hpp"
#include "logger.hpp"
#include "utils.hpp"

#ifndef _WIN32
#include <arpa/inet.h>
#include <netdb.h>
#include <sys/stat.h>
#else
#include <winsock2.h>
#include <ws2tcpip.h>
#endif
#include <fstream>
#include <set>
#include <sstream>
#include <utility>
#include <vector>

namespace ecnuvpn {
namespace tunnel {

static std::string extract_host(const std::string &server) {
  std::string value = server;
  std::size_t scheme_pos = value.find("://");
  std::size_t host_start = scheme_pos == std::string::npos ? 0 : scheme_pos + 3;
  std::size_t host_end = value.find_first_of("/?#", host_start);
  std::string authority = value.substr(host_start, host_end - host_start);
  if (authority.empty())
    return "";

  if (authority.front() == '[') {
    std::size_t close = authority.find(']');
    if (close != std::string::npos)
      return authority.substr(1, close - 1);
    return authority;
  }

  std::size_t colon = authority.rfind(':');
  if (colon != std::string::npos && authority.find(':') == colon)
    return authority.substr(0, colon);
  return authority;
}

static bool parse_ipv4(const std::string &value, uint32_t *out) {
  in_addr addr{};
  if (inet_pton(AF_INET, value.c_str(), &addr) != 1)
    return false;
  if (out)
    *out = ntohl(addr.s_addr);
  return true;
}

static std::vector<std::string> resolve_server_ips(const Config &cfg) {
  std::vector<std::string> result;
  std::string host = extract_host(cfg.server);
  if (host.empty())
    return result;

  std::set<std::string> unique;
  uint32_t literal = 0;
  if (parse_ipv4(host, &literal))
    unique.insert(host);

  addrinfo hints{};
  hints.ai_family = AF_INET;
  hints.ai_socktype = SOCK_STREAM;

  addrinfo *addresses = nullptr;
  if (getaddrinfo(host.c_str(), nullptr, &hints, &addresses) == 0) {
    for (addrinfo *cursor = addresses; cursor != nullptr; cursor = cursor->ai_next) {
      auto *sockaddr_v4 = reinterpret_cast<sockaddr_in *>(cursor->ai_addr);
      char buffer[INET_ADDRSTRLEN] = {0};
      if (inet_ntop(AF_INET, &sockaddr_v4->sin_addr, buffer, sizeof(buffer)))
        unique.insert(buffer);
    }
    freeaddrinfo(addresses);
  }

  result.assign(unique.begin(), unique.end());
  return result;
}

static bool route_contains_ip(const std::string &route, const std::string &ip) {
  uint32_t ip_value = 0;
  if (!parse_ipv4(ip, &ip_value))
    return false;

  std::size_t slash = route.find('/');
  if (slash == std::string::npos) {
    uint32_t route_ip = 0;
    return parse_ipv4(route, &route_ip) && route_ip == ip_value;
  }

  uint32_t network = 0;
  if (!parse_ipv4(route.substr(0, slash), &network))
    return false;

  int prefix = -1;
  try {
    prefix = std::stoi(route.substr(slash + 1));
  } catch (...) {
    return false;
  }

  if (prefix < 0 || prefix > 32)
    return false;

  uint32_t mask = prefix == 0 ? 0 : (~uint32_t(0) << (32 - prefix));
  return (network & mask) == (ip_value & mask);
}

static std::vector<std::string> find_server_route_exceptions(const Config &cfg) {
  std::vector<std::string> server_ips = resolve_server_ips(cfg);
  std::set<std::string> result;

  for (const auto &server_ip : server_ips) {
    for (const auto &route : cfg.routes) {
      if (route_contains_ip(route, server_ip)) {
        result.insert(server_ip);
        break;
      }
    }
  }

  return std::vector<std::string>(result.begin(), result.end());
}

static std::pair<std::string, std::string>
cidr_to_network_and_mask(const std::string &cidr) {
  std::size_t slash = cidr.find('/');
  std::string network = (slash == std::string::npos) ? cidr : cidr.substr(0, slash);
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

#ifdef _WIN32
static std::string js_quote(const std::string &value) {
  std::string quoted = "\"";
  for (char c : value) {
    switch (c) {
    case '\\':
      quoted += "\\\\";
      break;
    case '"':
      quoted += "\\\"";
      break;
    case '\r':
      quoted += "\\r";
      break;
    case '\n':
      quoted += "\\n";
      break;
    default:
      quoted += c;
      break;
    }
  }
  quoted += '"';
  return quoted;
}

static void append_windows_route_array(std::ostringstream &ss,
                                       const char *name,
                                       const std::vector<std::string> &routes) {
  ss << "var " << name << " = [\n";
  for (std::size_t i = 0; i < routes.size(); ++i) {
    auto [network, mask] = cidr_to_network_and_mask(routes[i]);
    ss << "  { cidr: " << js_quote(routes[i]) << ", network: "
       << js_quote(network) << ", mask: " << js_quote(mask) << " }";
    if (i + 1 != routes.size())
      ss << ",";
    ss << "\n";
  }
  ss << "];\n\n";
}

static void append_windows_string_array(std::ostringstream &ss,
                                        const char *name,
                                        const std::vector<std::string> &values) {
  ss << "var " << name << " = [\n";
  for (std::size_t i = 0; i < values.size(); ++i) {
    ss << "  " << js_quote(values[i]);
    if (i + 1 != values.size())
      ss << ",";
    ss << "\n";
  }
  ss << "];\n\n";
}

static std::string generate_windows(const Config &cfg) {
  std::ostringstream ss;
  std::vector<std::string> server_route_exceptions =
      find_server_route_exceptions(cfg);

  ss << "// =================================================================\n";
  ss << "// ECNU-VPN Split Tunnel Script (Auto-generated for Windows)\n";
  ss << "// Do NOT edit manually - regenerated on each VPN start\n";
  ss << "// =================================================================\n\n";
  ss << "var accumulatedExitCode = 0;\n";
  ss << "var ws = WScript.CreateObject(\"WScript.Shell\");\n";
  ss << "var env = ws.Environment(\"Process\");\n";
  ss << "var fs = WScript.CreateObject(\"Scripting.FileSystemObject\");\n";
  ss << "var readyFile = " << js_quote(utils::get_route_ready_path()) << ";\n";
  ss << "var debugFile = readyFile + '.debug.log';\n";
  append_windows_route_array(ss, "customRoutes", cfg.routes);
  append_windows_string_array(ss, "serverRouteExceptions",
                              server_route_exceptions);

  ss << "if (!String.prototype.trim) {\n";
  ss << "  String.prototype.trim = function () {\n";
  ss << "    return this.replace(/^[\\s\\uFEFF\\xA0]+|[\\s\\uFEFF\\xA0]+$/g, '');\n";
  ss << "  };\n";
  ss << "}\n\n";

  ss << "function envValue(name, fallback) {\n";
  ss << "  try {\n";
  ss << "    var value = env(name);\n";
  ss << "    if (value === undefined || value === null || value === '')\n";
  ss << "      return fallback;\n";
  ss << "    return value;\n";
  ss << "  } catch (e) {\n";
  ss << "    return fallback;\n";
  ss << "  }\n";
  ss << "}\n\n";

  ss << "function debugLog(message) {\n";
  ss << "  try {\n";
  ss << "    var file = fs.OpenTextFile(debugFile, 8, true);\n";
  ss << "    file.WriteLine((new Date()).toString() + ' ' + message);\n";
  ss << "    file.Close();\n";
  ss << "  } catch (e) {}\n";
  ss << "}\n\n";

  ss << "function isNumeric(value) {\n";
  ss << "  return /^[0-9]+$/.test(String(value));\n";
  ss << "}\n\n";

  ss << "function quoteArg(value) {\n";
  ss << "  value = String(value);\n";
  ss << "  if (value.length === 0) return '\"\"';\n";
  ss << "  return '\"' + value.replace(/([\"\\\\])/g, '\\\\$1') + '\"';\n";
  ss << "}\n\n";

  ss << "function psSingleQuote(value) {\n";
  ss << "  return '\\'' + String(value).replace(/'/g, \"''\") + '\\'';\n";
  ss << "}\n\n";

  ss << "function runCapture(cmd) {\n";
  ss << "  debugLog('capture: ' + cmd);\n";
  ss << "  try {\n";
  ss << "    var exec = ws.Exec(cmd);\n";
  ss << "    exec.StdIn.Close();\n";
  ss << "    return exec.StdOut.ReadAll() + exec.StdErr.ReadAll();\n";
  ss << "  } catch (e) {\n";
  ss << "    WScript.Echo('>>> [VPN] Command capture failed: ' + cmd + ' (' + (e.message || e.description || 'unknown') + ')');\n";
  ss << "    return '';\n";
  ss << "  }\n";
  ss << "}\n\n";

  ss << "function runExitCode(cmd) {\n";
  ss << "  debugLog('run: ' + cmd);\n";
  ss << "  try {\n";
  ss << "    return ws.Run(cmd, 0, true);\n";
  ss << "  } catch (e) {\n";
  ss << "    WScript.Echo('>>> [VPN] Command launch failed: ' + cmd + ' (' + (e.message || e.description || 'unknown') + ')');\n";
  ss << "    debugLog('launch failed: ' + cmd + ' (' + (e.message || e.description || 'unknown') + ')');\n";
  ss << "    return 1;\n";
  ss << "  }\n";
  ss << "}\n\n";

  ss << "function run(cmd, ignoreFailure) {\n";
  ss << "  var exitCode = runExitCode(cmd);\n";
  ss << "  if (exitCode !== 0 && !ignoreFailure) {\n";
  ss << "    accumulatedExitCode += exitCode;\n";
  ss << "    WScript.Echo('>>> [VPN] Command failed: ' + cmd + ' (exit ' + exitCode + ')');\n";
  ss << "  }\n";
  ss << "  return exitCode === 0;\n";
  ss << "}\n\n";

  ss << "function runWithRetry(cmd, maxRetries, delayMs, ignoreFailure) {\n";
  ss << "  for (var attempt = 1; attempt <= maxRetries; ++attempt) {\n";
  ss << "    var exitCode = runExitCode(cmd);\n";
  ss << "    debugLog('exit ' + exitCode + ': ' + cmd);\n";
  ss << "    if (exitCode === 0) return true;\n";
  ss << "    if (attempt < maxRetries) {\n";
  ss << "      WScript.Echo('>>> [VPN] Retry ' + attempt + '/' + maxRetries + ': ' + cmd + ' (exit ' + exitCode + ')');\n";
  ss << "      WScript.Sleep(delayMs);\n";
  ss << "    }\n";
  ss << "  }\n";
  ss << "  if (!ignoreFailure) {\n";
  ss << "    accumulatedExitCode += 1;\n";
  ss << "    WScript.Echo('>>> [VPN] Failed after ' + maxRetries + ' attempts: ' + cmd);\n";
  ss << "  }\n";
  ss << "  return false;\n";
  ss << "}\n\n";

  ss << "function getDefaultGateway4() {\n";
  ss << "  var output = runCapture('route.exe print 0.0.0.0');\n";
  ss << "  if (output.match(/0\\.0\\.0\\.0 *(0|128)\\.0\\.0\\.0 *([0-9\\.]*)/))\n";
  ss << "    return RegExp.$2;\n";
  ss << "  return '';\n";
  ss << "}\n\n";

  ss << "function getInterfaceIndex(adapterName) {\n";
  ss << "  if (!adapterName) return '';\n";
  ss << "  if (isNumeric(adapterName)) return adapterName;\n";
  ss << "  var ps = '(Get-NetAdapter -Name ' + psSingleQuote(adapterName) + ' -ErrorAction SilentlyContinue).ifIndex';\n";
  ss << "  var output = runCapture('powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command ' + quoteArg(ps));\n";
  ss << "  var idx = output.replace(/[\\s\\r\\n]/g, '');\n";
  ss << "  return isNumeric(idx) ? idx : '';\n";
  ss << "}\n\n";

  ss << "function netshAddressTarget(adapterName, ifIndex) {\n";
  ss << "  if (adapterName && !isNumeric(adapterName)) return 'name=' + quoteArg(adapterName);\n";
  ss << "  if (ifIndex) return ifIndex;\n";
  ss << "  return '';\n";
  ss << "}\n\n";

  ss << "function netshSubinterfaceTarget(adapterName, ifIndex) {\n";
  ss << "  if (adapterName && !isNumeric(adapterName)) return quoteArg(adapterName);\n";
  ss << "  if (ifIndex) return ifIndex;\n";
  ss << "  return '';\n";
  ss << "}\n\n";

  ss << "function deleteReadyFile() {\n";
  ss << "  debugLog('deleteReadyFile reason=' + envValue('reason', ''));\n";
  ss << "  try {\n";
  ss << "    if (fs.FileExists(readyFile))\n";
  ss << "      fs.DeleteFile(readyFile, true);\n";
  ss << "  } catch (e) {}\n";
  ss << "}\n\n";

  ss << "function writeReadyFile(tundev, internalIp) {\n";
  ss << "  debugLog('writeReadyFile tundev=' + tundev + ' ip=' + internalIp);\n";
  ss << "  try {\n";
  ss << "    var file = fs.OpenTextFile(readyFile, 2, true);\n";
  ss << "    file.WriteLine(tundev);\n";
  ss << "    file.WriteLine(internalIp);\n";
  ss << "    file.Close();\n";
  ss << "  } catch (e) {\n";
  ss << "    WScript.Echo('>>> [VPN] Failed to write route-ready marker: ' + e.message);\n";
  ss << "    WScript.Quit(1);\n";
  ss << "  }\n";
  ss << "}\n\n";

  ss << "function cleanupRoutes() {\n";
  ss << "  for (var i = 0; i < serverRouteExceptions.length; ++i)\n";
  ss << "    run('route.exe delete ' + serverRouteExceptions[i] + ' mask 255.255.255.255', true);\n";
  ss << "  for (var j = 0; j < customRoutes.length; ++j) {\n";
  ss << "    var route = customRoutes[j];\n";
  ss << "    run('route.exe delete ' + route.network + ' mask ' + route.mask, true);\n";
  ss << "  }\n";
  ss << "}\n\n";

  ss << "function preserveBypassRoutes(defaultGateway) {\n";
  ss << "  if (!defaultGateway)\n";
  ss << "    return;\n";
  ss << "  for (var i = 0; i < serverRouteExceptions.length; ++i) {\n";
  ss << "    var ip = serverRouteExceptions[i];\n";
  ss << "    run('route.exe delete ' + ip + ' mask 255.255.255.255', true);\n";
  ss << "    if (run('route.exe add ' + ip + ' mask 255.255.255.255 ' + defaultGateway, false))\n";
  ss << "      WScript.Echo('  [+] Server route preserved: ' + ip);\n";
  ss << "    else\n";
  ss << "      WScript.Echo('  [-] Server route warning: ' + ip + ' (failed to preserve upstream path)');\n";
  ss << "  }\n";
  ss << "}\n\n";

  ss << "function connectVpn() {\n";
  ss << "  debugLog('connect start TUNIDX=' + envValue('TUNIDX', '') + ' TUNDEV=' + envValue('TUNDEV', '') + ' IP=' + envValue('INTERNAL_IP4_ADDRESS', ''));\n";
  ss << "  deleteReadyFile();\n";
  ss << "  var tunidx = envValue('TUNIDX', '');\n";
  ss << "  var tundev = envValue('TUNDEV', tunidx);\n";
  ss << "  var internalIp = envValue('INTERNAL_IP4_ADDRESS', '');\n";
  ss << "  var netmask = envValue('INTERNAL_IP4_NETMASK', '255.255.255.255');\n";
  ss << "  if (!tunidx || !internalIp) {\n";
  ss << "    WScript.Echo('>>> [VPN] Missing Windows tunnel metadata from openconnect.');\n";
  ss << "    WScript.Quit(1);\n";
  ss << "  }\n\n";

  ss << "  var adapterName = tundev || tunidx;\n";
  ss << "  var ifIndex = isNumeric(tunidx) ? tunidx : getInterfaceIndex(adapterName);\n";
  ss << "  var addressTarget = netshAddressTarget(adapterName, ifIndex);\n";
  ss << "  var subinterfaceTarget = netshSubinterfaceTarget(adapterName, ifIndex);\n";
  ss << "  debugLog('resolved adapter=' + adapterName + ' ifIndex=' + ifIndex + ' addressTarget=' + addressTarget + ' subinterfaceTarget=' + subinterfaceTarget);\n";
  ss << "  if (!addressTarget) {\n";
  ss << "    WScript.Echo('>>> [VPN] Failed to resolve Windows tunnel interface: ' + adapterName);\n";
  ss << "    WScript.Quit(1);\n";
  ss << "  }\n\n";

  ss << "  var defaultGateway = getDefaultGateway4();\n";
  ss << "  WScript.Echo('>>> [VPN] Connection established, configuring network...');\n";
  ss << "  WScript.Echo('>>> [VPN] Adapter: ' + adapterName + ' | Index: ' + ifIndex + ' | Internal IP: ' + internalIp);\n";
  ss << "  if (defaultGateway)\n";
  ss << "    WScript.Echo('>>> [VPN] Default gateway: ' + defaultGateway);\n\n";

  ss << "  preserveBypassRoutes(defaultGateway);\n\n";

  ss << "  var mtu = envValue('INTERNAL_IP4_MTU', '');\n";
  ss << "  if (mtu && subinterfaceTarget)\n";
  ss << "    runWithRetry('netsh.exe interface ipv4 set subinterface ' + subinterfaceTarget + ' mtu=' + mtu + ' store=active', 3, 1000, false);\n\n";

  ss << "  runWithRetry('netsh.exe interface ipv4 set address ' + addressTarget + ' static ' + internalIp + ' ' + netmask, 5, 1000, false);\n\n";

  ss << "  // Wait for the interface IP to be fully registered before adding routes\n";
  ss << "  WScript.Sleep(3000);\n\n";

  ss << "  if (!ifIndex && adapterName && !isNumeric(adapterName)) {\n";
  ss << "    ifIndex = getInterfaceIndex(adapterName);\n";
  ss << "    if (ifIndex)\n";
  ss << "      WScript.Echo('>>> [VPN] Interface index resolved after IP setup: ' + ifIndex);\n";
  ss << "  }\n\n";

  ss << "  WScript.Echo('>>> [VPN] Adding split tunnel routes...');\n";
  ss << "  for (var i = 0; i < customRoutes.length; ++i) {\n";
  ss << "    var route = customRoutes[i];\n";
  ss << "    run('route.exe delete ' + route.network + ' mask ' + route.mask, true);\n";
  ss << "    var routeCmd = 'route.exe add ' + route.network + ' mask ' + route.mask + ' ' + internalIp;\n";
  ss << "    if (ifIndex) routeCmd += ' if ' + ifIndex;\n";
  ss << "    routeCmd += ' metric 1';\n";
  ss << "    if (runWithRetry(routeCmd, 5, 1000, false))\n";
  ss << "      WScript.Echo('  [+] Route added: ' + route.cidr);\n";
  ss << "    else\n";
  ss << "      WScript.Echo('  [-] Route warning: ' + route.cidr + ' (failed to refresh)');\n";
  ss << "  }\n\n";

  ss << "  if (accumulatedExitCode !== 0) {\n";
  ss << "    debugLog('network incomplete accumulatedExitCode=' + accumulatedExitCode);\n";
  ss << "    WScript.Echo('>>> [VPN] Network configuration incomplete; route-ready marker will not be written.');\n";
  ss << "    WScript.Quit(1);\n";
  ss << "  }\n\n";

  ss << "  writeReadyFile(tundev, internalIp);\n";
  ss << "  WScript.Echo('>>> [VPN] Network configuration complete!');\n";
  ss << "  WScript.Echo('>>> [Tip] Campus traffic via VPN, other traffic via default route.');\n";
  ss << "  WScript.Quit(0);\n";
  ss << "}\n\n";

  ss << "try {\n";
  ss << "  debugLog('script reason=' + envValue('reason', ''));\n";
  ss << "  switch (envValue('reason', '')) {\n";
  ss << "  case 'pre-init':\n";
  ss << "    deleteReadyFile();\n";
  ss << "    WScript.Quit(0);\n";
  ss << "  case 'disconnect':\n";
  ss << "    cleanupRoutes();\n";
  ss << "    deleteReadyFile();\n";
  ss << "    WScript.Quit(0);\n";
  ss << "  case 'reconnect':\n";
  ss << "  case 'attempt-reconnect':\n";
  ss << "    debugLog('keeping routes during transient reconnect state');\n";
  ss << "    WScript.Quit(0);\n";
  ss << "  case 'connect':\n";
  ss << "    connectVpn();\n";
  ss << "    break;\n";
  ss << "  default:\n";
  ss << "    WScript.Quit(0);\n";
  ss << "  }\n";
  ss << "} catch (e) {\n";
  ss << "  debugLog('script error: ' + (e.message || e.description || 'unknown'));\n";
  ss << "  WScript.Echo('>>> [VPN] Script error: ' + (e.message || e.description || 'unknown'));\n";
  ss << "  if (envValue('reason', '') === 'pre-init') WScript.Quit(0);\n";
  ss << "  WScript.Quit(1);\n";
  ss << "}\n";

  return ss.str();
}
#else
static std::string generate_posix(const Config &cfg) {
  std::ostringstream ss;
  std::vector<std::string> server_route_exceptions =
      find_server_route_exceptions(cfg);
  bool has_runtime_owner = utils::has_runtime_owner();
  uid_t runtime_owner_uid = utils::get_runtime_owner_uid();
  gid_t runtime_owner_gid = utils::get_runtime_owner_gid();

  ss << "#!/bin/bash\n";
  ss << "\n";
  ss << "# =================================================================\n";
  ss << "# ECNU-VPN Split Tunnel Script (Auto-generated)\n";
  ss << "# Do NOT edit manually - regenerated on each VPN start\n";
  ss << "# =================================================================\n";
  ss << "\n";
  ss << "READY_FILE=\"" << utils::get_route_ready_path() << "\"\n";
  if (has_runtime_owner) {
    ss << "OWNER_UID=\"" << runtime_owner_uid << "\"\n";
    ss << "OWNER_GID=\"" << runtime_owner_gid << "\"\n";
  }
  ss << "\n";

  // Bake route lists into the script so disconnect can clean them up
  // without depending on the route-ready file or config.
  ss << "CUSTOM_ROUTES=\"";
  for (std::size_t i = 0; i < cfg.routes.size(); ++i) {
    if (i > 0) ss << " ";
    ss << cfg.routes[i];
  }
  ss << "\"\n";
  ss << "SERVER_EXCEPTIONS=\"";
  for (std::size_t i = 0; i < server_route_exceptions.size(); ++i) {
    if (i > 0) ss << " ";
    ss << server_route_exceptions[i];
  }
  ss << "\"\n";
  ss << "\n";

  ss << "delete_ready_file() {\n";
  ss << "    rm -f \"$READY_FILE\"\n";
  ss << "}\n";
  ss << "\n";

  // Shell cleanup function — deletes all VPN routes from the OS table.
  ss << "cleanup_routes() {\n";
  ss << "    VPN_GW=\"$VPNGATEWAY\"\n";
  ss << "    if [ -n \"$VPN_GW\" ] && [ \"${VPN_GW%%:*}\" = \"$VPN_GW\" ]; then\n";
#ifdef __APPLE__
  ss << "        route -n delete \"$VPN_GW\" >/dev/null 2>&1\n";
#else
  ss << "        ip route del \"$VPN_GW\" >/dev/null 2>&1\n";
#endif
  ss << "    fi\n";
  ss << "    for ip in $SERVER_EXCEPTIONS; do\n";
#ifdef __APPLE__
  ss << "        route -n delete \"$ip\" >/dev/null 2>&1\n";
#else
  ss << "        ip route del \"$ip\" >/dev/null 2>&1\n";
#endif
  ss << "    done\n";
  ss << "    for route in $CUSTOM_ROUTES; do\n";
#ifdef __APPLE__
  ss << "        route -n delete \"$route\" >/dev/null 2>&1\n";
#else
  ss << "        ip route del \"$route\" >/dev/null 2>&1\n";
#endif
  ss << "    done\n";
  ss << "}\n";
  ss << "\n";

  ss << "case \"$reason\" in\n";
  ss << "    pre-init)\n";
  ss << "        delete_ready_file\n";
  ss << "        exit 0\n";
  ss << "        ;;\n";
  ss << "    disconnect|reconnect|attempt-reconnect)\n";
  ss << "        cleanup_routes\n";
  ss << "        delete_ready_file\n";
  ss << "        exit 0\n";
  ss << "        ;;\n";
  ss << "    connect)\n";
  ss << "        ;;\n";
  ss << "    *)\n";
  ss << "        exit 0\n";
  ss << "        ;;\n";
  ss << "esac\n";
  ss << "\n";

  ss << "echo \">>> [VPN] Connection established, configuring network...\"\n";
  ss << "echo \">>> [VPN] Interface: $TUNDEV | Internal IP: $INTERNAL_IP4_ADDRESS\"\n";
  ss << "\n";

  ss << "# Activate virtual interface\n";
#ifdef __APPLE__
  ss << "ifconfig \"$TUNDEV\" \"$INTERNAL_IP4_ADDRESS\" \"$INTERNAL_IP4_ADDRESS\" netmask 255.255.255.255 up >/dev/null 2>&1\n";
#else
  ss << "ip addr add \"$INTERNAL_IP4_ADDRESS/32\" dev \"$TUNDEV\" >/dev/null 2>&1\n";
  ss << "ip link set \"$TUNDEV\" up >/dev/null 2>&1\n";
#endif
  ss << "if [ $? -ne 0 ]; then\n";
  ss << "    echo \">>> [VPN] Failed to activate interface: $TUNDEV\"\n";
  ss << "    exit 1\n";
  ss << "fi\n";
  ss << "\n";

  if (!server_route_exceptions.empty()) {
#ifdef __APPLE__
    ss << "DEFAULT_ROUTE=$(route -n get default 2>/dev/null)\n";
    ss << "DEFAULT_GATEWAY=$(printf '%s\\n' \"$DEFAULT_ROUTE\" | awk '/gateway:/{print $2; exit}')\n";
    ss << "DEFAULT_INTERFACE=$(printf '%s\\n' \"$DEFAULT_ROUTE\" | awk '/interface:/{print $2; exit}')\n";
#else
    ss << "DEFAULT_GATEWAY=$(ip route show default 0.0.0.0/0 2>/dev/null | awk '{print $3; exit}')\n";
    ss << "DEFAULT_INTERFACE=$(ip route show default 0.0.0.0/0 2>/dev/null | awk '{print $5; exit}')\n";
#endif
    ss << "if [ -n \"$DEFAULT_GATEWAY\" ] && [ -n \"$DEFAULT_INTERFACE\" ]; then\n";
    for (const auto &server_ip : server_route_exceptions) {
#ifdef __APPLE__
      ss << "    route -n delete \"" << server_ip << "\" >/dev/null 2>&1\n";
      ss << "    route -n add -host \"" << server_ip
         << "\" \"$DEFAULT_GATEWAY\" >/dev/null 2>&1\n";
#else
      ss << "    ip route del \"" << server_ip << "\" >/dev/null 2>&1\n";
      ss << "    ip route add \"" << server_ip
         << "\" via \"$DEFAULT_GATEWAY\" >/dev/null 2>&1\n";
#endif
      ss << "    if [ $? -eq 0 ]; then\n";
      ss << "        echo \"  [+] Server route preserved: " << server_ip
         << " via $DEFAULT_INTERFACE\"\n";
      ss << "    else\n";
      ss << "        echo \"  [-] Server route warning: " << server_ip
         << " (failed to preserve upstream path)\"\n";
      ss << "    fi\n";
    }
    ss << "fi\n";
    ss << "\n";
  }

  ss << "# Split tunnel routes\n";
  ss << "echo \">>> [VPN] Adding split tunnel routes...\"\n";
  ss << "\n";

  for (const auto &route : cfg.routes) {
#ifdef __APPLE__
    ss << "route -n delete \"" << route << "\" >/dev/null 2>&1\n";
    ss << "route -n add \"" << route
       << "\" -interface \"$TUNDEV\" >/dev/null 2>&1\n";
#else
    ss << "ip route del \"" << route << "\" >/dev/null 2>&1\n";
    ss << "ip route add \"" << route
       << "\" dev \"$TUNDEV\" >/dev/null 2>&1\n";
#endif
    ss << "if [ $? -eq 0 ]; then\n";
    ss << "    echo \"  [+] Route added: " << route << "\"\n";
    ss << "else\n";
    ss << "    echo \"  [-] Route warning: " << route
       << " (failed to refresh)\"\n";
    ss << "fi\n";
    ss << "\n";
  }

  ss << "printf '%s\\n%s\\n' \"$TUNDEV\" \"$INTERNAL_IP4_ADDRESS\" > \"$READY_FILE\"\n";
  ss << "if [ $? -ne 0 ]; then\n";
  ss << "    echo \">>> [VPN] Failed to write route-ready marker.\"\n";
  ss << "    exit 1\n";
  ss << "fi\n";
  if (has_runtime_owner) {
    ss << "chown \"$OWNER_UID\":\"$OWNER_GID\" \"$READY_FILE\" >/dev/null 2>&1\n";
    ss << "chmod 0644 \"$READY_FILE\" >/dev/null 2>&1\n";
  }
  ss << "\n";
  ss << "echo \">>> [VPN] Network configuration complete!\"\n";
  ss << "echo \">>> [Tip] Campus traffic via VPN, other traffic via default route.\"\n";
  ss << "\n";
  ss << "exit 0\n";

  return ss.str();
}
#endif

std::string generate(const Config &cfg) {
#ifdef _WIN32
  return generate_windows(cfg);
#else
  return generate_posix(cfg);
#endif
}

bool write_script(const Config &cfg) {
  std::string path = utils::get_tunnel_path();
  std::string content = generate(cfg);

  for (const auto &server_ip : find_server_route_exceptions(cfg)) {
    logger::warn("Preserving upstream route for VPN server IP: " + server_ip);
  }

  if (!utils::write_file(path, content)) {
    utils::print_error("Failed to write tunnel script: " + path);
    logger::error("Failed to write tunnel script: " + path);
    return false;
  }

#ifndef _WIN32
  if (chmod(path.c_str(), 0755) != 0) {
    utils::print_error("Failed to set executable permission on: " + path);
    logger::error("Failed to chmod tunnel script: " + path);
    return false;
  }
#endif

  utils::sync_owner(path);

  logger::info("Tunnel script generated: " + path);
  return true;
}

void cleanup_routes() {
#ifndef _WIN32

  Config cfg = config::load();
  if (cfg.routes.empty()) {
    logger::info("No routes configured, skipping route cleanup");
    return;
  }
  std::vector<std::string> server_exceptions = find_server_route_exceptions(cfg);
  logger::info("Cleaning up VPN routes (" + std::to_string(cfg.routes.size()) +
               " configured)");

#ifdef __APPLE__
  for (const auto &route : cfg.routes) {
    utils::run_command("route -n delete " + utils::shell_quote(route) +
                       " >/dev/null 2>&1");
  }
  for (const auto &ip : server_exceptions) {
    utils::run_command("route -n delete " + utils::shell_quote(ip) +
                       " >/dev/null 2>&1");
  }
#else
  for (const auto &route : cfg.routes) {
    utils::run_command("ip route del " + utils::shell_quote(route) +
                       " >/dev/null 2>&1");
  }
  for (const auto &ip : server_exceptions) {
    utils::run_command("ip route del " + utils::shell_quote(ip) +
                       " >/dev/null 2>&1");
  }
#endif

  logger::info("Route cleanup complete (" +
               std::to_string(cfg.routes.size()) + " routes removed)");
#endif
}

} // namespace tunnel
} // namespace ecnuvpn
