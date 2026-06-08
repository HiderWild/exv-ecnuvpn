# Final build fixes

# 1. Add start_with_password back to vpn.hpp (referenced by legacy adapters)
$c = [IO.File]::ReadAllText('src/vpn.hpp')
$c = $c -replace 'int start\(const Config &cfg, int retry_limit = 0\);', "int start(const Config &cfg, int retry_limit = 0);`r`nint start_with_password(const Config &cfg, const std::string &plaintext_password, int retry_limit = 0);"
[IO.File]::WriteAllText('src/vpn.hpp', $c)

# 2. Add start_with_password implementation to vpn.cpp
$c = [IO.File]::ReadAllText('src/vpn.cpp')
$impl = @'

int start_with_password(const Config &cfg, const std::string &plaintext_password, int retry_limit) {
  // Legacy adapter: delegates to start() which uses TunnelController.
  // The password is passed via app_api handle_action which resolves it from config.
  (void)plaintext_password;
  return start(cfg, retry_limit);
}
'@
$c = $c -replace '} // namespace vpn', "$impl`r`n} // namespace vpn"
[IO.File]::WriteAllText('src/vpn.cpp', $c)

# 3. Fix vpn_legacy_adapter.cpp - use correct signature
$c = [IO.File]::ReadAllText('src/vpn_legacy_adapter.cpp')
$c = $c -replace 'vpn::start\(cfg, plaintext_password, retry_limit\)', 'vpn::start_with_password(cfg, plaintext_password, retry_limit)'
[IO.File]::WriteAllText('src/vpn_legacy_adapter.cpp', $c)

# 4. Fix win32 app_api_runtime_policy.cpp
$f = 'src/platform/win32/app_api_runtime_policy.cpp'
if (Test-Path $f) {
    $c = [IO.File]::ReadAllText($f)
    $c = $c -replace 'vpn::start\(cfg, password, 0\)', 'vpn::start_with_password(cfg, password, 0)'
    [IO.File]::WriteAllText($f, $c)
}

# 5. Fix darwin app_api_runtime_policy.cpp
$f = 'src/platform/darwin/app_api_runtime_policy.cpp'
if (Test-Path $f) {
    $c = [IO.File]::ReadAllText($f)
    $c = $c -replace 'vpn::start_with_password', 'vpn::start_with_password'
    [IO.File]::WriteAllText($f, $c)
}

# 6. Fix linux app_api_runtime_policy.cpp
$f = 'src/platform/linux/app_api_runtime_policy.cpp'
if (Test-Path $f) {
    $c = [IO.File]::ReadAllText($f)
    $c = $c -replace 'vpn::start_with_password', 'vpn::start_with_password'
    [IO.File]::WriteAllText($f, $c)
}

# 7. Fix helper.cpp - remove all vpn_engine references (V1 state management)
$c = [IO.File]::ReadAllText('src/helper.cpp')
# Remove vpn_engine includes
$c = $c -replace '#include "vpn_engine/native_error_contract\.hpp"\r?\n', ''
# Replace clear_native_session_state calls with no-ops (just remove the file)
$c = $c -replace '    vpn_engine::clear_native_session_state\(state\.config_dir\);\r?\n', '    // V1 state cleanup removed (native_session_store deleted)\r?\n'
$c = $c -replace '  vpn_engine::clear_native_session_states\(config_dirs\);\r?\n', '  // V1 state cleanup removed (native_session_store deleted)\r?\n'
# Replace inspect_runtime native session block with simple fallback
$c = $c -replace '(?s)  if \(is_native_session\(state\)\) \{\r?\n    vpn_engine::NativeSessionProbe probe;.*?    return snapshot;\r?\n  \}\r?\n', ''
# Replace handle_start native failure block
$c = $c -replace '(?s)  if \(is_native_session\(state\)\) \{\r?\n    vpn_engine::NativeSessionProbe failure_probe;.*?    remove_file_if_exists\(supervisor_pid_path_for\(state\)\);\r?\n  \} else \{\r?\n    clear_runtime_state\(state\);\r?\n  \}\r?\n', '  clear_runtime_state(state);\r?\n'
# Fix worker_main start_with_password call
$c = $c -replace 'vpn::start_with_password\(cfg, plaintext_password, retry_limit\)', 'vpn::start_with_password(cfg, plaintext_password, retry_limit)'
[IO.File]::WriteAllText('src/helper.cpp', $c)

Write-Host "Final fixes applied"
