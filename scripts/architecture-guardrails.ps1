# architecture-guardrails.ps1 - Verify architecture boundaries (PowerShell)
# Idempotent and safe to run multiple times.

$ErrorActionPreference = "Continue"
$failures = 0

Write-Host "=== Architecture Guardrails ==="

# Resolve repo root (parent of scripts/)
$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir
Set-Location $repoRoot

# Helper function to check pattern in directory
function Check-Pattern {
    param(
        [string]$Description,
        [string[]]$Paths,
        [string]$Pattern,
        [string[]]$ExcludePatterns = @(),
        [switch]$IsWarning
    )

    Write-Host -NoNewline "$Description... "

    $found = $false
    foreach ($path in $Paths) {
        if (-not (Test-Path $path)) { continue }
        $files = Get-ChildItem -Path $path -Recurse -File -ErrorAction SilentlyContinue
        foreach ($file in $files) {
            $content = Get-Content $file.FullName -ErrorAction SilentlyContinue
            foreach ($line in $content) {
                if ($line -match $Pattern) {
                    $excluded = $false
                    foreach ($ex in $ExcludePatterns) {
                        if ($line -match $ex) { $excluded = $true; break }
                    }
                    if (-not $excluded) {
                        $found = $true
                        Write-Host ""
                        Write-Host "  $($file.FullName): $line"
                    }
                }
            }
        }
    }

    if ($found) {
        if ($IsWarning) {
            Write-Host "WARNING"
        } else {
            Write-Host "FAIL"
            $script:failures++
        }
    } else {
        Write-Host "PASS"
    }
}

# 1. Helper V2 must not contain password/cookie/token
Check-Pattern -Description "Checking helper for forbidden fields" `
    -Paths "src/helper_common", "src/helper_runtime" `
    -Pattern "(?i)password|cookie|webvpn_session|auth_token" `
    -ExcludePatterns "^\\s*//", "test", "\.md"

# 2. Helper V2 must not include vpn_engine/protocol
Check-Pattern -Description "Checking helper for protocol includes" `
    -Paths "src/helper_common", "src/helper_runtime" `
    -Pattern "vpn_engine/protocol"

# 3. Core must not have platform #ifdef
Check-Pattern -Description "Checking core for platform ifdef" `
    -Paths "src/core" `
    -Pattern "#ifdef _WIN32|#ifdef __APPLE__|#if defined\(_WIN32\)|#if defined\(__APPLE__\)"

# 4. New UI path must not have retry_limit
Check-Pattern -Description "Checking new UI path for retry_limit" `
    -Paths "src/core_api" `
    -Pattern "retry_limit"

# 5. No secrets in test fixtures
Check-Pattern -Description "Checking test fixtures for secrets" `
    -Paths "tests" `
    -Pattern "(?i)password\s*=\s*['""]" `
    -ExcludePatterns "no_secret", "forbidden", "should_not", "must_not", "verify_no", "// " `
    -IsWarning

# 6. Helper must not include core/ headers
Check-Pattern -Description "Checking helper for core includes" `
    -Paths "src/helper_common", "src/helper_runtime" `
    -Pattern '#include.*"core/'

# 7. Platform ops must not include protocol/auth headers
Check-Pattern -Description "Checking platform ops for protocol includes" `
    -Paths "src/platform" `
    -Pattern "vpn_engine/protocol|native_auth"

Write-Host ""
if ($failures -gt 0) {
    Write-Host "=== $failures guardrail violations found ==="
    exit 1
} else {
    Write-Host "=== All guardrails passed ==="
    exit 0
}
