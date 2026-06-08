# Fix all remaining include and reference issues
$ErrorActionPreference = 'Stop'

# 1. Remove dead includes from helper.cpp
$c = [IO.File]::ReadAllText('src/helper.cpp')
$c = $c -replace '#include "vpn_engine/native_session_store\.hpp"\r?\n', ''
[IO.File]::WriteAllText('src/helper.cpp', $c)

# 2. Remove dead includes from app_api.cpp  
$c = [IO.File]::ReadAllText('src/app_api.cpp')
$c = $c -replace '#include "vpn_engine/native_session_store\.hpp"\r?\n', ''
[IO.File]::WriteAllText('src/app_api.cpp', $c)

# 3. Rewrite main.cpp - remove webui include and all webui code blocks
$c = [IO.File]::ReadAllText('src/main.cpp')
# Remove include
$c = $c -replace '#include "webui\.hpp"\r?\n', ''
# Remove webui global vars
$c = $c -replace 'static volatile sig_atomic_t webui_stop_requested = 0;\r?\n', ''
$c = $c -replace 'static webui::WebUIServer \*g_webui_server = nullptr;\r?\n', ''
$c = $c -replace 'static void webui_signal_handler\(int\) \{ webui_stop_requested = 1; \}\r?\n', ''
# Remove the entire webui_enabled block (from "if (cfg.webui_enabled)" to the closing brace before next cmd check)
$c = $c -replace '(?s)\r?\n    if \(cfg\.webui_enabled\).*?(?=\r?\n  if \(cmd == "stop")', ''
# Remove webui_enabled check in start
$c = $c -replace '(?s)    // Warn if WebUI is enabled.*?    \}\r?\n\r?\n', ''
# Remove foreground flag
$c = $c -replace '  bool foreground = false;\r?\n', ''
$c = $c -replace '    if \(arg == "--foreground" \|\| arg == "-f"\) \{\r?\n      parsed\.foreground = true;\r?\n      continue;\r?\n    \}\r?\n', ''
# Remove -f from help
$c = $c -replace '  std::cout << "  " << utils::YELLOW << "-f, --foreground" << utils::RESET\r?\n.*?<< std::endl;\r?\n', ''
$c = $c -replace '(?s)#ifdef __APPLE__\r?\n  std::cout << "  Note: Desktop app is the recommended interface.*?<< std::endl;\r?\n#endif\r?\n', ''
[IO.File]::WriteAllText('src/main.cpp', $c)

Write-Host "All fixes applied"
