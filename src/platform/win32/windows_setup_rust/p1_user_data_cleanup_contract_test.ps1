$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$setupRoot = Split-Path -Parent $PSCommandPath
$repoRoot = $setupRoot
1..4 | ForEach-Object { $repoRoot = Split-Path -Parent $repoRoot }
$cleanupScript = Join-Path $repoRoot 'scripts\clear-local-user-config.ps1'
$packageScript = Join-Path $repoRoot 'scripts\package-rust-setup.ps1'

function Assert-Contains([string]$Text, [string]$Pattern, [string]$Message) {
  if ($Text -notmatch $Pattern) {
    throw "EXPECT FAILED: $Message; pattern=$Pattern"
  }
}

function Assert-PathMissing([string]$Path, [string]$Message) {
  if (Test-Path -LiteralPath $Path) {
    throw "EXPECT FAILED: $Message ($Path)"
  }
}

$cleanup = Get-Content -LiteralPath $cleanupScript -Raw
$package = Get-Content -LiteralPath $packageScript -Raw
$uninstall = Get-Content -LiteralPath (Join-Path $setupRoot 'uninstall_engine.cpp') -Raw

# The installer must carry the executable cleanup policy.  Otherwise the uninstall
# engine silently reports that the support script was unavailable.
Assert-Contains -Text $package -Pattern "'support\\clear-local-user-config\.ps1'" -Message 'Rust setup payload includes the clear-user-data support script'
Assert-Contains -Text $uninstall -Pattern 'cleanup_result\.started' -Message 'uninstall reports a failed clear-user-data script instead of claiming success'
Assert-Contains -Text $uninstall -Pattern 'cleanup_result\.exit_code != 0' -Message 'uninstall treats a nonzero clear-user-data script exit as failure'
Assert-Contains -Text $uninstall -Pattern 'AppendError\(errors' -Message 'uninstall records missing cleanup-script errors'

# Current Rust state is intentionally under %USERPROFILE%\.exv; the old WebView
# profile lives below %LOCALAPPDATA%\EXV.  Both roots must be explicit cleanup
# targets when the user elects to clear data.
Assert-Contains -Text $cleanup -Pattern 'rustConfigDir.*\.exv' -Message 'cleanup resolves the current Rust config root'
Assert-Contains -Text $cleanup -Pattern 'rustConfigDir.*userProfile' -Message 'cleanup deletes the current Rust config root'
Assert-Contains -Text $cleanup -Pattern 'appRoot.*localAppData' -Message 'cleanup deletes the legacy EXV app-data root'
Assert-Contains -Text $cleanup -Pattern 'machineResourceRoot.*programData' -Message 'cleanup deletes the machine resource journal and log root'

# Exercise the real script against isolated directories.  NoProcessStop ensures the
# contract test cannot stop a developer's active EXV process.
$testRoot = Join-Path ([IO.Path]::GetTempPath()) ("exv-uninstall-user-data-{0}" -f [guid]::NewGuid())
$localAppData = Join-Path $testRoot 'LocalAppData'
$userProfile = Join-Path $testRoot 'UserProfile'
$programData = Join-Path $testRoot 'ProgramData'
$oldUserProfile = $env:USERPROFILE
$oldLocalAppData = $env:LOCALAPPDATA
$oldProgramData = $env:ProgramData

try {
  $markers = @(
    (Join-Path $userProfile '.exv\config.json'),
    (Join-Path $userProfile '.exv\key.bin'),
    (Join-Path $userProfile '.exv\logs\engine.jsonl'),
    (Join-Path $localAppData 'EXV\profile\default\ui-preferences.json'),
    (Join-Path $localAppData 'ExvVpn\journal\state.json'),
    (Join-Path $programData 'ExvVpn\logs\engine.jsonl'),
    (Join-Path $programData 'exv\service.key')
  )
  foreach ($marker in $markers) {
    New-Item -ItemType Directory -Path (Split-Path -Parent $marker) -Force | Out-Null
    Set-Content -LiteralPath $marker -Value 'EXV test state' -NoNewline
  }

  $env:USERPROFILE = $userProfile
  $env:LOCALAPPDATA = $localAppData
  $env:ProgramData = $programData
  & $cleanupScript -Force -NoProcessStop -LocalAppDataRoot $localAppData -ProgramDataRoot $programData `
    -TempRoot (Join-Path $testRoot 'Temp') -SystemTempRoot (Join-Path $testRoot 'SystemTemp') *> $null
  $cleanupSucceeded = $?
  if (-not $cleanupSucceeded) {
    throw 'EXPECT FAILED: cleanup script reported failure'
  }

  Assert-PathMissing (Join-Path $userProfile '.exv') 'current Rust user state survives clear-user-data'
  Assert-PathMissing (Join-Path $localAppData 'EXV') 'legacy EXV user state survives clear-user-data'
  Assert-PathMissing (Join-Path $localAppData 'ExvVpn') 'local fallback journal/log state survives clear-user-data'
  Assert-PathMissing (Join-Path $programData 'ExvVpn') 'machine journal/log state survives clear-user-data'
  Assert-PathMissing (Join-Path $programData 'exv') 'machine service state survives clear-user-data'
} finally {
  $env:USERPROFILE = $oldUserProfile
  $env:LOCALAPPDATA = $oldLocalAppData
  $env:ProgramData = $oldProgramData
  if (Test-Path -LiteralPath $testRoot) {
    Remove-Item -LiteralPath $testRoot -Recurse -Force
  }
}

Write-Output 'p1_user_data_cleanup_contract_test: ok'
