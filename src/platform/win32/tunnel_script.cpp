#include "platform/common/tunnel_script.hpp"

#include "platform/common/process_utils.hpp"

#include <winsock2.h>
#include <ws2tcpip.h>
#include <windows.h>

#include "observability/log_facade.hpp"
#include "vpn_engine/openconnect/openconnect_log.hpp"

#include <chrono>
#include <cstdint>
#include <cstdlib>
#include <cstdio>
#include <fstream>
#include <regex>
#include <sstream>
#include <string>
#include <utility>
#include <vector>

namespace ecnuvpn {
namespace platform {
namespace {
// Begin inlined from platform/win32/tunnel_script_timing include-unit
class TunnelTiming {
public:
  TunnelTiming() : started_(Clock::now()), last_(started_) {
    exv::observability::LogFacade::info("[connect-timing] scope=tunnel.windows stage=begin delta_ms=0 total_ms=0");
  }

  void mark(const std::string &stage, const std::string &detail = "") {
    auto now = Clock::now();
    auto delta_ms = elapsed_ms(last_, now);
    auto total_ms = elapsed_ms(started_, now);
    last_ = now;
    std::string message = "[connect-timing] scope=tunnel.windows stage=" +
                          stage + " delta_ms=" + std::to_string(delta_ms) +
                          " total_ms=" + std::to_string(total_ms);
    if (!detail.empty())
      message += " " + detail;
    exv::observability::LogFacade::info(message);
  }

  void finish(bool ok, const std::string &detail = "") {
    if (finished_)
      return;
    finished_ = true;
    mark(ok ? "finish.ok" : "finish.error", detail);
  }

private:
  using Clock = std::chrono::steady_clock;

  static long long elapsed_ms(const Clock::time_point &from,
                              const Clock::time_point &to) {
    return std::chrono::duration_cast<std::chrono::milliseconds>(to - from)
        .count();
  }

  Clock::time_point started_;
  Clock::time_point last_;
  bool finished_ = false;
};
// End inlined from platform/win32/tunnel_script_timing include-unit
// Begin inlined from platform/win32/tunnel_script_helpers include-unit
std::string js_quote(const std::string &value) {
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

std::string env_value(const char *name, const std::string &fallback = "") {
  const char *value = std::getenv(name);
  if (!value || !*value)
    return fallback;
  return value;
}

bool is_numeric(const std::string &value) {
  if (value.empty())
    return false;
  for (char c : value) {
    if (c < '0' || c > '9')
      return false;
  }
  return true;
}

std::string cmd_quote_arg(const std::string &value) {
  std::string quoted = "\"";
  for (char c : value) {
    if (c == '"')
      quoted += "\\\"";
    else
      quoted += c;
  }
  quoted += "\"";
  return quoted;
}

void debug_log(const std::string &ready_path, const std::string &message) {
  std::ofstream out(ready_path + ".debug.log", std::ios::app);
  if (out.is_open())
    out << message << "\n";
}

int run_exit(const std::string &ready_path, const std::string &cmd) {
  debug_log(ready_path, "run: " + cmd);
  int rc = platform::run_command(cmd);
  debug_log(ready_path, "exit " + std::to_string(rc) + ": " + cmd);
  return rc;
}

bool run_with_retry(const std::string &ready_path, const std::string &cmd,
                    int max_retries, unsigned int delay_ms,
                    bool ignore_failure) {
  for (int attempt = 1; attempt <= max_retries; ++attempt) {
    int rc = run_exit(ready_path, cmd);
    if (rc == 0)
      return true;
    if (attempt < max_retries)
      Sleep(delay_ms);
  }
  return ignore_failure;
}

std::string effective_mtu(const std::string &reported_mtu, int configured_mtu) {
  int reported = 0;
  try {
    reported = reported_mtu.empty() ? 0 : std::stoi(reported_mtu);
  } catch (...) {
    reported = 0;
  }

  if (reported >= 1200)
    return std::to_string(reported);
  if (configured_mtu >= 1200)
    return std::to_string(configured_mtu);
  return "";
}

std::string get_default_gateway4() {
  std::string output = platform::run_command_output("route.exe print 0.0.0.0");
  std::regex route_regex(R"(0\.0\.0\.0\s+(?:0|128)\.0\.0\.0\s+([0-9.]+))");
  std::smatch match;
  if (std::regex_search(output, match, route_regex) && match.size() > 1)
    return match[1].str();
  return "";
}

void delete_ready_file(const std::string &ready_path) {
  std::remove(ready_path.c_str());
}

bool write_ready_file(const std::string &ready_path, const std::string &tundev,
                      const std::string &internal_ip) {
  std::ofstream out(ready_path, std::ios::trunc);
  if (!out.is_open())
    return false;
  out << tundev << "\n" << internal_ip << "\n";
  return true;
}

std::pair<std::string, std::string>
cidr_to_network_and_mask(const std::string &cidr);
// End inlined from platform/win32/tunnel_script_helpers include-unit
// Begin inlined from platform/win32/tunnel_script_configure include-unit
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
// End inlined from platform/win32/tunnel_script_configure include-unit
// Begin inlined from platform/win32/tunnel_script_generator include-unit
void append_windows_route_array(std::ostringstream &ss, const char *name,
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

void append_windows_string_array(std::ostringstream &ss, const char *name,
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

} // namespace

std::string generate_tunnel_script(const TunnelScriptContext &context) {
  std::ostringstream ss;

  ss << "// =================================================================\n";
  ss << "// ECNU-VPN Split Tunnel Script (Auto-generated for Windows)\n";
  ss << "// Do NOT edit manually - regenerated on each VPN start\n";
  ss << "// =================================================================\n\n";
  ss << "var accumulatedExitCode = 0;\n";
  ss << "var ws = WScript.CreateObject(\"WScript.Shell\");\n";
  ss << "var env = ws.Environment(\"Process\");\n";
  ss << "var fs = WScript.CreateObject(\"Scripting.FileSystemObject\");\n";
  ss << "var readyFile = " << js_quote(context.route_ready_path) << ";\n";
  ss << "var debugFile = readyFile + '.debug.log';\n";
  ss << "var configuredMtu = " << context.configured_mtu << ";\n";
  append_windows_route_array(ss, "customRoutes", context.custom_routes);
  append_windows_string_array(ss, "serverRouteExceptions",
                              context.server_route_exceptions);

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

  ss << "function effectiveMtu(reportedMtu) {\n";
  ss << "  var reported = parseInt(reportedMtu || '0', 10);\n";
  ss << "  if (reported >= 1200) return String(reported);\n";
  ss << "  if (configuredMtu >= 1200) {\n";
  ss << "    if (reportedMtu)\n";
  ss << "      debugLog('ignoring low reported MTU=' + reportedMtu + ', using configured MTU=' + configuredMtu);\n";
  ss << "    return String(configuredMtu);\n";
  ss << "  }\n";
  ss << "  return '';\n";
  ss << "}\n\n";

  ss << "function quoteArg(value) {\n";
  ss << "  value = String(value);\n";
  ss << "  if (value.length === 0) return '\"\"';\n";
  ss << "  return '\"' + value.replace(/([\"\\\\])/g, '\\\\$1') + '\"';\n";
  ss << "}\n\n";

  ss << "function psSingleQuote(value) {\n";
  ss << "  return '\'' + String(value).replace(/'/g, \"''\") + '\'';\n";
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

  ss << "  var mtu = effectiveMtu(envValue('INTERNAL_IP4_MTU', ''));\n";
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
// End inlined from platform/win32/tunnel_script_generator include-unit
// Begin inlined from platform/win32/tunnel_script_runtime include-unit
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
// End inlined from platform/win32/tunnel_script_runtime include-unit
} // namespace platform
} // namespace ecnuvpn
