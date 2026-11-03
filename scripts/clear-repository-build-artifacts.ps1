param(
  [string]$SharedArtifactRoot = '',
  [switch]$KeepLatestInstaller,
  [switch]$WhatIf,
  [switch]$LibraryOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Test-IsWithin([string]$path, [string]$parent) {
  $resolvedPath = [IO.Path]::GetFullPath($path)
  $resolvedParent = [IO.Path]::GetFullPath($parent).TrimEnd('\')
  return $resolvedPath.StartsWith($resolvedParent + '\', [StringComparison]::OrdinalIgnoreCase)
}

function Assert-ExistingWorktreePath([string]$path, [string]$worktree) {
  $resolved = [IO.Path]::GetFullPath($path)
  $resolvedWorktree = [IO.Path]::GetFullPath($worktree).TrimEnd('\')
  if ($resolved -ne $resolvedWorktree -and -not (Test-IsWithin $resolved $resolvedWorktree)) {
    throw "Refusing to access a path outside its registered worktree: $resolved"
  }
  $current = $resolvedWorktree
  $worktreeItem = Get-Item -LiteralPath $current -Force -ErrorAction Stop
  if (($worktreeItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Refusing to traverse a registered worktree that is a reparse point: $current"
  }
  $components = $resolved.Substring($resolvedWorktree.Length).TrimStart('\').Split('\')
  foreach ($component in $components) {
    if ([string]::IsNullOrWhiteSpace($component)) { continue }
    $current = Join-Path $current $component
    $item = Get-Item -LiteralPath $current -Force -ErrorAction Stop
    if (($item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
      throw "Refusing to traverse a reparse point below a registered worktree: $current"
    }
  }
  return $resolved
}

function Assert-ArtifactPath([string]$path, [string]$worktree) {
  $resolved = Assert-ExistingWorktreePath $path $worktree
  if ($resolved -eq [IO.Path]::GetFullPath($worktree).TrimEnd('\')) {
    throw "Refusing to clean a worktree root: $resolved"
  }
  return $resolved
}

function Assert-ArtifactCreationPath([string]$path, [string]$worktree) {
  $resolved = [IO.Path]::GetFullPath($path)
  if (-not (Test-IsWithin $resolved $worktree)) {
    throw "Refusing to create a build prerequisite outside its registered worktree: $resolved"
  }
  $parent = Split-Path -Parent $resolved
  $null = Assert-ExistingWorktreePath $parent $worktree
  return $resolved
}

if ($LibraryOnly) { return }

$repoRoot = [IO.Path]::GetFullPath((Split-Path -Parent $PSScriptRoot))
. (Join-Path $PSScriptRoot 'shared-artifact-root.ps1')
$canonicalSharedArtifactRoot = Get-ExvSharedArtifactRoot -RepositoryRoot $repoRoot
if ([string]::IsNullOrWhiteSpace($SharedArtifactRoot)) {
  $SharedArtifactRoot = $canonicalSharedArtifactRoot
}
$SharedArtifactRoot = Assert-ExvCanonicalSharedArtifactRoot `
  -RequestedRoot $SharedArtifactRoot `
  -CanonicalRoot $canonicalSharedArtifactRoot

$records = @(& git -C $repoRoot worktree list --porcelain)
if ($LASTEXITCODE -ne 0) { throw 'Unable to enumerate registered Git worktrees.' }
$worktrees = @(
  $records | Where-Object { $_ -match '^worktree (.+)$' } |
    ForEach-Object { [IO.Path]::GetFullPath($Matches[1]) }
)
if ($worktrees.Count -eq 0) { throw 'Git returned no registered worktrees.' }
foreach ($worktree in $worktrees) {
  if (Test-IsWithin $SharedArtifactRoot $worktree -or $SharedArtifactRoot -eq $worktree) {
    throw "Shared artifact root must be outside every repository worktree: $SharedArtifactRoot"
  }
}

$artifactPaths = New-Object 'System.Collections.Generic.List[string]'
$installers = New-Object 'System.Collections.Generic.List[System.IO.FileInfo]'
$preparedWindowsRustTempRoots = New-Object 'System.Collections.Generic.List[string]'
$nestedOutputs = @(
  'src\platform\win32\rust\.build-tmp',
  'src\platform\win32\rust\target',
  'src\platform\win32\rust\tauri\target',
  'src\platform\win32\rust\tauri\frontend\dist'
)

foreach ($worktree in $worktrees) {
  if (-not (Test-Path -LiteralPath $worktree -PathType Container)) { continue }
  Get-ChildItem -LiteralPath $worktree -Force -ErrorAction Stop |
    Where-Object {
      ($_.PSIsContainer -and ($_.Name -match '^build' -or $_.Name -in @('.build-tmp', 'target', 'dist', 'out'))) -or
      (-not $_.PSIsContainer -and $_.Name -match '^EXV-.+-windows-x64-setup\.exe$')
    } |
    ForEach-Object {
      if ($_.PSIsContainer) { $artifactPaths.Add((Assert-ArtifactPath $_.FullName $worktree)) }
      else { $installers.Add($_) }
    }
  foreach ($relative in $nestedOutputs) {
    $candidate = Join-Path $worktree $relative
    if (Test-Path -LiteralPath $candidate) { $artifactPaths.Add((Assert-ArtifactPath $candidate $worktree)) }
  }
  Get-ChildItem -LiteralPath $worktree -Recurse -File -Filter 'EXV-*-windows-x64-setup.exe' -Force -ErrorAction SilentlyContinue |
    ForEach-Object { $installers.Add($_) }
}

# Codex-hosted builds can leave these target/cache directories in the root
# worktree. They are build products, not repository state, and follow the
# same unconditional cleanup rule.
foreach ($relative in @('.codex-cargo-target', '.codex-npm-cache', '.codex-tmp')) {
  $candidate = Join-Path $repoRoot $relative
  if (Test-Path -LiteralPath $candidate) {
    $artifactPaths.Add((Assert-ArtifactPath $candidate $repoRoot))
  }
}

$artifactPaths = @($artifactPaths | Sort-Object -Unique)
$installers = @($installers | Sort-Object FullName -Unique)
if ($KeepLatestInstaller) {
  Invoke-ExvCanonicalInstallerRetention `
    -Worktrees $worktrees `
    -SharedArtifactRoot $SharedArtifactRoot `
    -WhatIf:$WhatIf
}

foreach ($path in $artifactPaths) {
  if (Test-Path -LiteralPath $path) {
    if ($WhatIf) { Write-Host "Would remove build artifact: $path" }
    else {
      Write-Host "Removing build artifact: $path"
      Remove-Item -LiteralPath $path -Recurse -Force -ErrorAction Stop
    }
  }
}

# Cargo force-sets TMP/TEMP to this directory for Windows Rust worktrees. Recreate it empty
# after cleanup: it is a build prerequisite, not a retained build artifact.
foreach ($worktree in $worktrees) {
  $rustRoot = Join-Path $worktree 'src\platform\win32\rust'
  $cargoConfig = Join-Path $rustRoot '.cargo\config.toml'
  if (-not (Test-Path -LiteralPath $cargoConfig -PathType Leaf)) { continue }
  $tempRoot = Assert-ArtifactCreationPath (Join-Path $rustRoot '.build-tmp') $worktree
  if ($WhatIf) {
    Write-Host "Would create empty Windows Rust build temp root: $tempRoot"
    continue
  }
  New-Item -ItemType Directory -Force -Path $tempRoot | Out-Null
  $tempRootItem = Get-Item -LiteralPath $tempRoot -Force -ErrorAction Stop
  if (($tempRootItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "Refusing to retain a Windows Rust build temp root that is a reparse point: $tempRoot"
  }
  if (@(Get-ChildItem -LiteralPath $tempRoot -Force).Count -ne 0) {
    throw "Windows Rust build temp root was not empty after cleanup: $tempRoot"
  }
  $preparedWindowsRustTempRoots.Add($tempRoot)
}
foreach ($installer in $installers) {
  if (Test-Path -LiteralPath $installer.FullName) {
    $worktree = $worktrees | Where-Object { Test-IsWithin $installer.FullName $_ } | Select-Object -First 1
    $null = Assert-ArtifactPath $installer.FullName $worktree
    if ($WhatIf) { Write-Host "Would remove installer copy: $($installer.FullName)" }
    else {
      Write-Host "Removing installer copy: $($installer.FullName)"
      Remove-Item -LiteralPath $installer.FullName -Force -ErrorAction Stop
    }
  }
}

Write-Host "Build artifact cleanup complete. mode=$($(if ($WhatIf) { 'WHATIF' } else { 'APPLY' })) worktrees=$($worktrees.Count) directories=$($artifactPaths.Count) installers=$($installers.Count) prepared_windows_rust_temp_roots=$($preparedWindowsRustTempRoots.Count) shared=$SharedArtifactRoot"
