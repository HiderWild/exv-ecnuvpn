# Fix all remaining build errors

# 1. main.cpp: add #include <optional>
$c = [IO.File]::ReadAllText('src/main.cpp')
$c = $c -replace '#include <string>', "#include <optional>`r`n#include <string>"
[IO.File]::WriteAllText('src/main.cpp', $c)

# 2. vpn.cpp: add missing include for vpn_engine
$c = [IO.File]::ReadAllText('src/vpn.cpp')
$c = "#include `"vpn_engine/native_engine.hpp`"`r`n" + $c
[IO.File]::WriteAllText('src/vpn.cpp', $c)

# 3. vpn.hpp: add back start_with_password and kUseTunnelController (referenced by other files)
$c = [IO.File]::ReadAllText('src/vpn.hpp')
$c = $c -replace '// Start the VPN connection via TunnelController', "inline constexpr int kUseTunnelController = 3;`r`n`r`nint start_with_password(const Config &cfg, const std::string &plaintext_password, int retry_limit = 0);`r`n`r`n// Start the VPN connection via TunnelController"
[IO.File]::WriteAllText('src/vpn.hpp', $c)

# 4. app_api.cpp: remove cleanup_legacy_supervisor_state_files calls
$c = [IO.File]::ReadAllText('src/app_api.cpp')
$c = $c -replace '      cleanup_legacy_supervisor_state_files\(\);\r?\n', ''
[IO.File]::WriteAllText('src/app_api.cpp', $c)

# 5. helper.cpp: remove all references to vpn_engine::NativeSessionProbe/Snapshot/clear_native_session_state
$c = [IO.File]::ReadAllText('src/helper.cpp')
$c = $c -replace '#include "vpn_engine/native_error_contract\.hpp"\r?\n', ''
[IO.File]::WriteAllText('src/helper.cpp', $c)

# 6. vpn_legacy_adapter.cpp: replace start_with_password call
$c = [IO.File]::ReadAllText('src/vpn_legacy_adapter.cpp')
$c = $c -replace 'vpn::start_with_password', 'vpn::start'
[IO.File]::WriteAllText('src/vpn_legacy_adapter.cpp', $c)

# 7. app_api_runtime_policy.cpp (win32): replace start_with_password
$f = 'src/platform/win32/app_api_runtime_policy.cpp'
if (Test-Path $f) {
    $c = [IO.File]::ReadAllText($f)
    $c = $c -replace 'vpn::start_with_password', 'vpn::start'
    $c = $c -replace 'vpn::kUseTunnelController', 'vpn::kVpnInitialConnectFailedExitCode'
    [IO.File]::WriteAllText($f, $c)
}

Write-Host "All fixes applied"
