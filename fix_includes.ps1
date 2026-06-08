$files = @('src/main.cpp', 'src/helper.cpp', 'src/app_api.cpp')
foreach ($f in $files) {
    $lines = Get-Content $f
    $new = @()
    foreach ($line in $lines) {
        if ($line -match 'webui\.hpp' -or $line -match 'native_session_store\.hpp' -or $line -match 'sse_broadcaster\.hpp') {
            continue
        }
        $new += $line
    }
    [IO.File]::WriteAllLines($f, $new)
}
Write-Host "Done"
