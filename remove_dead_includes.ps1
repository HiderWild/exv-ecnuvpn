$files = @('src/main.cpp','src/vpn_runtime.cpp','src/helper.cpp','src/vpn.cpp','src/app_api.cpp')
foreach ($f in $files) {
    $content = [IO.File]::ReadAllText($f)
    $content = $content -replace '#include "sse_broadcaster.hpp"\r?\n', ''
    $content = $content -replace '#include "webui.hpp"\r?\n', ''
    $content = $content -replace '#include "vpn_engine/native_session_store.hpp"\r?\n', ''
    [IO.File]::WriteAllText($f, $content)
}
Write-Host "Removed dead includes from all files"
