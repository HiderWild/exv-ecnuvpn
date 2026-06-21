param(
  [ValidatePattern('^$|^[0-9]+(\.[0-9]+){2,3}$')]
  [string]$Version = "",
  [ValidatePattern('^$|^[0-9A-Za-z][0-9A-Za-z._+-]*$')]
  [string]$BuildLabel = "",
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

function Get-CmakeProjectVersion {
  param([Parameter(Mandatory = $true)][string]$CMakeListsPath)

  if (-not (Test-Path -LiteralPath $CMakeListsPath -PathType Leaf)) {
    throw "CMakeLists.txt not found: $CMakeListsPath"
  }

  $content = Get-Content -LiteralPath $CMakeListsPath -Raw
  $match = [regex]::Match(
    $content,
    '(?m)^\s*project\s*\(\s*exv\s+VERSION\s+([0-9]+(?:\.[0-9]+){2,3})\b'
  )
  if (-not $match.Success) {
    throw "Unable to read project(exv VERSION ...) from $CMakeListsPath"
  }

  return $match.Groups[1].Value
}

function Assert-ProductVersion {
  param([Parameter(Mandatory = $true)][string]$Value)

  if ($Value -notmatch '^[0-9]+(\.[0-9]+){2,3}$') {
    throw "Product version must be a CMake-compatible numeric version such as 3.3.0: $Value"
  }
}

function Join-ArtifactVersion {
  param(
    [Parameter(Mandatory = $true)][string]$Version,
    [string]$BuildLabel = ""
  )

  if ([string]::IsNullOrWhiteSpace($BuildLabel)) {
    return $Version
  }

  return "$Version-$BuildLabel"
}

function Invoke-Step {
  param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath,
    [string[]]$Arguments = @()
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
  $environmentRoot = 'D:\Development\Environment'
  if (Test-Path -LiteralPath $environmentRoot -PathType Container) {
    $nsisDirs = @(Get-ChildItem -LiteralPath $environmentRoot -Directory -Filter 'NSIS-*' -ErrorAction SilentlyContinue)
    foreach ($nsisDir in $nsisDirs) {
      [void]$candidates.Add((Join-Path $nsisDir.FullName 'Bin\makensis.exe'))
      [void]$candidates.Add((Join-Path $nsisDir.FullName 'makensis.exe'))
    }
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
  Invoke-Step -FilePath 'python' -Arguments @(
    $verifyScript,
    '--verify-launch-targets-only',
    '--platform',
    'windows',
    '--package-dir',
    $Root
  )
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

function Get-RunningExvProcesses {
  $filter = "Name='exv.exe' OR Name='exv-ui.exe' OR Name='exv-helper.exe'"
  return @(Get-CimInstance Win32_Process -Filter $filter -ErrorAction SilentlyContinue)
}

function Stop-PackageSmokeProcesses {
  param(
    [Parameter(Mandatory = $true)][string]$Root,
    [int[]]$KnownProcessIds = @()
  )

  $rootFull = [System.IO.Path]::GetFullPath($Root).TrimEnd('\', '/')
  $known = @{}
  foreach ($processId in $KnownProcessIds) {
    $known[[int]$processId] = $true
  }

  foreach ($process in Get-RunningExvProcesses) {
    $processId = [int]$process.ProcessId
    if ($known.ContainsKey($processId)) {
      continue
    }

    $exePath = [string]$process.ExecutablePath
    $commandLine = [string]$process.CommandLine
    $matchesPackageRoot = $false
    if (-not [string]::IsNullOrWhiteSpace($exePath)) {
      try {
        $exeFull = [System.IO.Path]::GetFullPath($exePath)
        $matchesPackageRoot = $exeFull.StartsWith($rootFull + '\', [System.StringComparison]::OrdinalIgnoreCase)
      } catch { }
    }
    if (-not $matchesPackageRoot -and
        -not [string]::IsNullOrWhiteSpace($commandLine)) {
      $matchesPackageRoot = $commandLine.IndexOf($rootFull, [System.StringComparison]::OrdinalIgnoreCase) -ge 0
    }

    # Some short-lived MinGW processes have already lost queryable image
    # metadata by cleanup time. If they were not present before smoke and have
    # an EXV binary name, treat them as smoke children so the temp package can
    # be removed.
    $unknownImage = [string]::IsNullOrWhiteSpace($exePath) -and
      [string]::IsNullOrWhiteSpace($commandLine)
    if ($matchesPackageRoot -or $unknownImage) {
      Stop-Process -Id $processId -Force -ErrorAction SilentlyContinue
    }
  }
}

function Remove-DirectoryWithRetry {
  param(
    [Parameter(Mandatory = $true)][string]$Path,
    [Parameter(Mandatory = $true)][string]$ProcessRoot,
    [int[]]$KnownProcessIds = @()
  )

  for ($attempt = 1; $attempt -le 6; $attempt++) {
    Stop-PackageSmokeProcesses -Root $ProcessRoot -KnownProcessIds $KnownProcessIds
    try {
      Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction Stop
      return
    } catch {
      if ($attempt -eq 6) {
        throw
      }
      Start-Sleep -Milliseconds (200 * $attempt)
    }
  }
}

function Test-PortableZip {
  param([Parameter(Mandatory = $true)][string]$Archive)

  $tempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("exv-portable-" + [System.Guid]::NewGuid().ToString('N'))
  New-Item -ItemType Directory -Path $tempRoot | Out-Null
  $knownExvProcessIds = @(Get-RunningExvProcesses | ForEach-Object { [int]$_.ProcessId })

  try {
    Expand-Archive -LiteralPath $Archive -DestinationPath $tempRoot -Force
    $children = @(Get-ChildItem -LiteralPath $tempRoot)
    if ($children.Count -ne 1 -or -not $children[0].PSIsContainer -or $children[0].Name -ne 'EXV') {
      $names = $children | ForEach-Object { $_.Name }
      throw "Portable zip must contain exactly one top-level EXV directory. Found: $($names -join ', ')"
    }

    $extractedPackage = Join-Path $tempRoot 'EXV'
    $smoke = Join-Path $repoRoot 'scripts\windows-packaging-smoke.ps1'
    Invoke-Step -FilePath 'powershell.exe' -Arguments @(
      '-NoProfile',
      '-ExecutionPolicy',
      'Bypass',
      '-File',
      $smoke,
      '-PackageRoot',
      $extractedPackage
    )
  }
  finally {
    if (Test-Path -LiteralPath $tempRoot) {
      Remove-DirectoryWithRetry -Path $tempRoot -ProcessRoot $tempRoot -KnownProcessIds $knownExvProcessIds
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
  Invoke-Step -FilePath $MakeNsis -Arguments @(
    '/V2',
    '/INPUTCHARSET',
    'UTF8',
    "/DAPP_VERSION=$AppVersion",
    "/DSOURCE_DIR=$SourceDir",
    "/DOUTPUT_FILE=$OutputFile",
    "/DDEFAULT_INSTALL_DIR=$defaultInstallDir",
    "/DUNINSTALL_MANIFEST=$manifestPath",
    $scriptPath
  )
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

$productVersion = if ([string]::IsNullOrWhiteSpace($Version)) {
  Get-CmakeProjectVersion -CMakeListsPath (Join-Path $repoRoot 'CMakeLists.txt')
} else {
  $Version
}
Assert-ProductVersion $productVersion
$artifactVersion = Join-ArtifactVersion -Version $productVersion -BuildLabel $BuildLabel

if (-not $SkipBuild) {
  $buildScript = Join-Path $repoRoot 'scripts\build-windows.ps1'
  Invoke-Step -FilePath 'powershell.exe' -Arguments @(
    '-NoProfile',
    '-ExecutionPolicy',
    'Bypass',
    '-File',
    $buildScript,
    'desktop'
  )
}

Assert-PackageRoot $resolvedPackageRoot
New-Item -ItemType Directory -Path $resolvedOutputDir -Force | Out-Null

Invoke-PackageVerifier $resolvedPackageRoot

$portableZip = Join-Path $resolvedOutputDir "EXV-$artifactVersion-windows-x64-portable.zip"
$installerExe = Join-Path $resolvedOutputDir "EXV-$artifactVersion-windows-x64-setup.exe"

New-PortableZip -Root $resolvedPackageRoot -Destination $portableZip
Test-PortableZip -Archive $portableZip

$makeNsis = Resolve-MakeNsis $NsisPath
Invoke-Nsis -MakeNsis $makeNsis -SourceDir $resolvedPackageRoot -OutputFile $installerExe -AppVersion $productVersion
Assert-InstallerOutput $installerExe

Write-Host ''
Write-Host 'Windows release artifacts:' -ForegroundColor Cyan
Write-Host "  Product version: $productVersion"
if (-not [string]::IsNullOrWhiteSpace($BuildLabel)) {
  Write-Host "  Build label: $BuildLabel"
}
Write-Host "  Portable: $portableZip"
Write-Host "  Installer: $installerExe"
