param(
  [Parameter(Mandatory = $true)]
  [ValidatePattern('^[0-9A-Za-z][0-9A-Za-z._+-]*$')]
  [string]$Version,
  [switch]$SkipBuild,
  [string]$PackageRoot = "",
  [string]$OutputDir = "",
  [string]$NsisPath = $env:NSIS_MAKENSIS
)

$ErrorActionPreference = 'Stop'

$scriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$repoRoot = Split-Path -Parent $scriptDir

function Resolve-AbsolutePath {
  param([Parameter(Mandatory = $true)][string]$Path)

  if ([System.IO.Path]::IsPathRooted($Path)) {
    return [System.IO.Path]::GetFullPath($Path)
  }

  return [System.IO.Path]::GetFullPath((Join-Path $repoRoot $Path))
}

function Invoke-Step {
  param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath,
    [Parameter(ValueFromRemainingArguments = $true)]
    [string[]]$Arguments
  )

  & $FilePath @Arguments
  if ($LASTEXITCODE -ne 0) {
    throw "Command failed: $FilePath $($Arguments -join ' ')"
  }
}

function Resolve-MakeNsis {
  param([string]$RequestedPath)

  $candidates = New-Object System.Collections.Generic.List[string]

  if (-not [string]::IsNullOrWhiteSpace($RequestedPath)) {
    if (Test-Path -LiteralPath $RequestedPath -PathType Container) {
      [void]$candidates.Add((Join-Path $RequestedPath 'makensis.exe'))
    }
    else {
      [void]$candidates.Add($RequestedPath)
    }
  }

  $pathCommand = Get-Command makensis.exe -ErrorAction SilentlyContinue
  if ($pathCommand) {
    [void]$candidates.Add($pathCommand.Source)
  }

  if ($env:ProgramFiles) {
    [void]$candidates.Add((Join-Path $env:ProgramFiles 'NSIS\makensis.exe'))
  }
  if (${env:ProgramFiles(x86)}) {
    [void]$candidates.Add((Join-Path ${env:ProgramFiles(x86)} 'NSIS\makensis.exe'))
  }

  foreach ($candidate in $candidates) {
    if (-not [string]::IsNullOrWhiteSpace($candidate) -and
        (Test-Path -LiteralPath $candidate -PathType Leaf)) {
      return (Resolve-Path -LiteralPath $candidate).Path
    }
  }

  throw 'makensis.exe was not found. Install NSIS, add makensis.exe to PATH, set NSIS_MAKENSIS, or pass -NsisPath.'
}

function Assert-PackageRoot {
  param([Parameter(Mandatory = $true)][string]$Root)

  if (-not (Test-Path -LiteralPath $Root -PathType Container)) {
    throw "Package root does not exist: $Root"
  }

  $required = @(
    'exv-ui.exe',
    'exv-ui.args',
    'bin\exv.exe',
    'bin\exv-helper.exe',
    'webui\index.html',
    'WebView2Loader.dll'
  )

  $missing = @()
  foreach ($relative in $required) {
    $candidate = Join-Path $Root $relative
    if (-not (Test-Path -LiteralPath $candidate -PathType Leaf)) {
      $missing += $relative
    }
  }

  if ($missing.Count -gt 0) {
    throw "Package root is missing required file(s): $($missing -join ', ')"
  }
}

function Invoke-PackageVerifier {
  param([Parameter(Mandatory = $true)][string]$Root)

  $verifyScript = Join-Path $repoRoot 'scripts\package_ui_shell.py'
  Invoke-Step python $verifyScript --verify-launch-targets-only --platform windows --package-dir $Root
}

function New-PortableZip {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Destination
  )

  if (Test-Path -LiteralPath $Destination) {
    Remove-Item -LiteralPath $Destination -Force
  }

  Compress-Archive -Path $Root -DestinationPath $Destination -CompressionLevel Optimal
  if (-not (Test-Path -LiteralPath $Destination -PathType Leaf)) {
    throw "Portable zip was not created: $Destination"
  }
}

function Test-PortableZip {
  param([Parameter(Mandatory = $true)][string]$Archive)

  $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("exv-portable-" + [System.Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $tempRoot | Out-Null

  try {
    Expand-Archive -LiteralPath $Archive -DestinationPath $tempRoot -Force
    $children = @(Get-ChildItem -LiteralPath $tempRoot)
    if ($children.Count -ne 1 -or -not $children[0].PSIsContainer -or $children[0].Name -ne 'EXV') {
      $names = $children | ForEach-Object { $_.Name }
      throw "Portable zip must contain exactly one top-level EXV directory. Found: $($names -join ', ')"
    }

    $extractedPackage = Join-Path $tempRoot 'EXV'
    $smoke = Join-Path $repoRoot 'scripts\windows-packaging-smoke.ps1'
    Invoke-Step powershell -NoProfile -ExecutionPolicy Bypass -File $smoke -PackageRoot $extractedPackage
  }
  finally {
    if (Test-Path -LiteralPath $tempRoot) {
      Remove-Item -LiteralPath $tempRoot -Recurse -Force
    }
  }
}

function Convert-ToNsisPath {
  param([Parameter(Mandatory = $true)][string]$Path)

  return ($Path -replace '/', '\')
}

function Get-RelativePathFromRoot {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [Parameter(Mandatory = $true)][string]$Child
  )

  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd(
    [System.IO.Path]::DirectorySeparatorChar,
    [System.IO.Path]::AltDirectorySeparatorChar
  )
  $childFull = [System.IO.Path]::GetFullPath($Child)
  $prefix = $rootFull + [System.IO.Path]::DirectorySeparatorChar
  if (-not $childFull.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Path is not under source root: $Child"
  }

  return $childFull.Substring($prefix.Length)
}

function New-NsisUninstallManifest {
  param(
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [Parameter(Mandatory = $true)][string]$Destination
  )

  $sourceRoot = (Resolve-Path -LiteralPath $SourceDir).Path
  $lines = New-Object System.Collections.Generic.List[string]
  [void]$lines.Add('; Generated by scripts\package-windows-release.ps1. Do not edit.')

  $files = Get-ChildItem -LiteralPath $sourceRoot -Recurse -File | Sort-Object FullName
  foreach ($file in $files) {
    $relative = Get-RelativePathFromRoot -Root $sourceRoot -Child $file.FullName
    $nsisPath = Convert-ToNsisPath $relative
    [void]$lines.Add("Delete `"`$INSTDIR\$nsisPath`"")
  }

  [void]$lines.Add('Delete "$INSTDIR\Uninstall.exe"')

  $directories = Get-ChildItem -LiteralPath $sourceRoot -Recurse -Directory | Sort-Object { $_.FullName.Length } -Descending
  foreach ($directory in $directories) {
    $relative = Get-RelativePathFromRoot -Root $sourceRoot -Child $directory.FullName
    $nsisPath = Convert-ToNsisPath $relative
    [void]$lines.Add("RMDir `"`$INSTDIR\$nsisPath`"")
  }

  [void]$lines.Add('RMDir "$INSTDIR"')
  Set-Content -LiteralPath $Destination -Value $lines -Encoding UTF8
}

function Invoke-Nsis {
  param(
    [Parameter(Mandatory = $true)][string]$MakeNsis,
    [Parameter(Mandatory = $true)][string]$SourceDir,
    [Parameter(Mandatory = $true)][string]$OutputFile,
    [Parameter(Mandatory = $true)][string]$AppVersion
  )

  $scriptPath = Join-Path $repoRoot 'distribution\windows\exv.nsi'
  if (-not (Test-Path -LiteralPath $scriptPath -PathType Leaf)) {
    throw "NSIS script not found: $scriptPath"
  }

  $manifestPath = [System.IO.Path]::ChangeExtension($OutputFile, '.uninstall.nsh')
  New-NsisUninstallManifest -SourceDir $SourceDir -Destination $manifestPath

  $defaultInstallDir = Join-Path $env:LOCALAPPDATA 'Programs\EXV'
  Invoke-Step $MakeNsis `
    /V2 `
    "/DAPP_VERSION=$AppVersion" `
    "/DSOURCE_DIR=$SourceDir" `
    "/DOUTPUT_FILE=$OutputFile" `
    "/DDEFAULT_INSTALL_DIR=$defaultInstallDir" `
    "/DUNINSTALL_MANIFEST=$manifestPath" `
    $scriptPath
}

function Assert-InstallerOutput {
  param([Parameter(Mandatory = $true)][string]$Installer)

  if (-not (Test-Path -LiteralPath $Installer -PathType Leaf)) {
    throw "Installer was not created: $Installer"
  }

  $item = Get-Item -LiteralPath $Installer
  if ($item.Length -le 0) {
    throw "Installer is empty: $Installer"
  }
}

$defaultPackageRoot = Join-Path $repoRoot 'build\windows\webview\package\EXV'
$resolvedPackageRoot = if ([string]::IsNullOrWhiteSpace($PackageRoot)) {
  $defaultPackageRoot
} else {
  Resolve-AbsolutePath $PackageRoot
}

$resolvedOutputDir = if ([string]::IsNullOrWhiteSpace($OutputDir)) {
  Join-Path $repoRoot 'build\windows\release'
} else {
  Resolve-AbsolutePath $OutputDir
}

if (-not $SkipBuild) {
  $buildScript = Join-Path $repoRoot 'scripts\build-windows.ps1'
  Invoke-Step powershell -NoProfile -ExecutionPolicy Bypass -File $buildScript desktop
}

Assert-PackageRoot $resolvedPackageRoot
New-Item -ItemType Directory -Path $resolvedOutputDir -Force | Out-Null

Invoke-PackageVerifier $resolvedPackageRoot

$portableZip = Join-Path $resolvedOutputDir "EXV-$Version-windows-x64-portable.zip"
$installerExe = Join-Path $resolvedOutputDir "EXV-$Version-windows-x64-setup.exe"

New-PortableZip -Root $resolvedPackageRoot -Destination $portableZip
Test-PortableZip -Archive $portableZip

$makeNsis = Resolve-MakeNsis $NsisPath
Invoke-Nsis -MakeNsis $makeNsis -SourceDir $resolvedPackageRoot -OutputFile $installerExe -AppVersion $Version
Assert-InstallerOutput $installerExe

Write-Host ''
Write-Host 'Windows release artifacts:' -ForegroundColor Cyan
Write-Host "  Portable: $portableZip"
Write-Host "  Installer: $installerExe"
