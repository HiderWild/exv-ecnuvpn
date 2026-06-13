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
