# run-tests.ps1 - Local test runner for ECNU-VPN
param(
    [string]$Preset = "windows-release",
    [string]$Label = "",
    [switch]$Verbose
)

$ErrorActionPreference = "Stop"

Write-Host "=== ECNU-VPN Test Runner ===" -ForegroundColor Cyan
Write-Host "Preset: $Preset"

# Configure
Write-Host "`n--- Configuring ---" -ForegroundColor Yellow
cmake --preset $Preset
if ($LASTEXITCODE -ne 0) { throw "CMake configure failed" }

# Build
Write-Host "`n--- Building ---" -ForegroundColor Yellow
cmake --build --preset $Preset
if ($LASTEXITCODE -ne 0) { throw "Build failed" }

# Test
Write-Host "`n--- Running Tests ---" -ForegroundColor Yellow
$ctestArgs = @("--preset", $Preset, "--output-on-failure")
if ($Label) {
    $ctestArgs += "-L"
    $ctestArgs += $Label
}
ctest @ctestArgs
$testResult = $LASTEXITCODE

# Summary
Write-Host "`n=== Results ===" -ForegroundColor Cyan
if ($testResult -eq 0) {
    Write-Host "All tests passed!" -ForegroundColor Green
} else {
    Write-Host "Some tests failed. Check Testing/Temporary/LastTest.log" -ForegroundColor Red
}

exit $testResult
