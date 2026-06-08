$files = @('src/helper.cpp', 'src/app_api.cpp')
foreach ($f in $files) {
    $lines = [IO.File]::ReadAllLines($f)
    $new = [System.Collections.ArrayList]::new()
    foreach ($line in $lines) {
        if ($line -match 'native_session_store\.hpp') { continue }
        [void]$new.Add($line)
    }
    [IO.File]::WriteAllLines($f, $new)
}
Write-Host "Fixed helper.cpp and app_api.cpp"
