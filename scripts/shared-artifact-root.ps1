Set-StrictMode -Version Latest

function Test-ExvSamePath([string]$Left, [string]$Right) {
  return [string]::Equals(
    [IO.Path]::GetFullPath($Left).TrimEnd('\'),
    [IO.Path]::GetFullPath($Right).TrimEnd('\'),
    [StringComparison]::OrdinalIgnoreCase
  )
}

function Test-ExvPathWithin([string]$Path, [string]$Parent) {
  $fullPath = [IO.Path]::GetFullPath($Path)
  $fullParent = [IO.Path]::GetFullPath($Parent)
  return $fullPath.StartsWith($fullParent.TrimEnd('\') + '\', [StringComparison]::OrdinalIgnoreCase)
}

function Get-ExvSharedArtifactRoot {
  param(
    [Parameter(Mandatory = $true)]
    [string]$RepositoryRoot,
    [string]$GitApplication = 'git'
  )

  $repoRoot = [IO.Path]::GetFullPath($RepositoryRoot)
  $gitCommon = @(& $GitApplication -C $repoRoot rev-parse --path-format=absolute --git-common-dir)
  if ($LASTEXITCODE -ne 0 -or $gitCommon.Count -ne 1 -or [string]::IsNullOrWhiteSpace($gitCommon[0])) {
    throw "Unable to resolve Git common directory for repository: $repoRoot"
  }
  $commonDir = [IO.Path]::GetFullPath($gitCommon[0].Trim())
  $mainWorktree = [IO.Path]::GetFullPath((Split-Path -Parent $commonDir))

  $records = @(& $GitApplication -C $repoRoot worktree list --porcelain)
  if ($LASTEXITCODE -ne 0) {
    throw "Unable to enumerate registered Git worktrees for repository: $repoRoot"
  }
  $worktrees = @(
    $records | Where-Object { $_ -match '^worktree (.+)$' } |
      ForEach-Object { [IO.Path]::GetFullPath($Matches[1].Trim()) }
  )
  if ($worktrees.Count -eq 0) {
    throw "Git returned no registered worktrees for repository: $repoRoot"
  }
  if (-not (@($worktrees | Where-Object { Test-ExvSamePath $_ $mainWorktree }).Count -eq 1)) {
    throw "Git common directory did not identify one registered main worktree: $mainWorktree"
  }

  $sharedRoot = [IO.Path]::GetFullPath((Join-Path (Split-Path -Parent $mainWorktree) 'exv-artifacts'))
  foreach ($worktree in $worktrees) {
    if ((Test-ExvSamePath $sharedRoot $worktree) -or (Test-ExvPathWithin $sharedRoot $worktree)) {
      throw "Shared artifact root must be outside every registered worktree: $sharedRoot"
    }
  }
  return $sharedRoot
}

function Get-ExvPackageOutputRoot {
  param(
    [Parameter(Mandatory = $true)]
    [string]$RepositoryRoot,
    [string]$RequestedOutputRoot = '',
    [string]$GitApplication = 'git'
  )

  if ([string]::IsNullOrWhiteSpace($RequestedOutputRoot)) {
    return Get-ExvSharedArtifactRoot -RepositoryRoot $RepositoryRoot -GitApplication $GitApplication
  }
  if ([IO.Path]::IsPathRooted($RequestedOutputRoot)) {
    return [IO.Path]::GetFullPath($RequestedOutputRoot)
  }
  return [IO.Path]::GetFullPath((Join-Path $RepositoryRoot $RequestedOutputRoot))
}

function Assert-ExvCanonicalSharedArtifactRoot {
  param(
    [Parameter(Mandatory = $true)]
    [string]$RequestedRoot,
    [Parameter(Mandatory = $true)]
    [string]$CanonicalRoot
  )

  $requested = [IO.Path]::GetFullPath($RequestedRoot)
  $canonical = [IO.Path]::GetFullPath($CanonicalRoot)
  if (-not (Test-ExvSamePath $requested $canonical)) {
    throw "SharedArtifactRoot must equal the canonical shared artifact root: $canonical"
  }
  return $canonical
}

function Test-ExvValidInstaller([System.IO.FileInfo]$Installer) {
  if ($null -eq $Installer -or -not $Installer.Exists -or $Installer.Length -lt 2) {
    return $false
  }
  if (($Installer.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    return $false
  }
  $stream = [IO.File]::Open($Installer.FullName, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
  try {
    return $stream.ReadByte() -eq 0x4d -and $stream.ReadByte() -eq 0x5a
  } finally {
    $stream.Dispose()
  }
}

function Get-ExvExistingItem([string]$Path, [string]$Label) {
  try {
    return Get-Item -LiteralPath $Path -Force -ErrorAction Stop
  } catch [System.Management.Automation.ItemNotFoundException] {
    return $null
  } catch [System.IO.FileNotFoundException] {
    return $null
  } catch [System.IO.DirectoryNotFoundException] {
    return $null
  } catch {
    throw "$Label is inaccessible: $Path ($($_.Exception.Message))"
  }
}

function Assert-ExvItemWithoutReparsePoint([IO.FileSystemInfo]$Item, [string]$Label) {
  if (($Item.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
    throw "$Label contains a reparse point: $($Item.FullName)"
  }
}

function Assert-ExvNoReparsePoint([string]$Path, [string]$Label) {
  $rootItem = Get-ExvExistingItem $Path $Label
  if ($null -ne $rootItem) {
    Assert-ExvItemWithoutReparsePoint $rootItem $Label
    if (($rootItem.Attributes -band [IO.FileAttributes]::Directory) -ne 0) {
      $pending = New-Object 'System.Collections.Generic.Stack[System.IO.DirectoryInfo]'
      $pending.Push([IO.DirectoryInfo]$rootItem)
      while ($pending.Count -gt 0) {
        $directory = $pending.Pop()
        try {
          $children = @($directory.GetFileSystemInfos())
        } catch {
          throw "$Label contains an inaccessible directory: $($directory.FullName) ($($_.Exception.Message))"
        }
        foreach ($child in $children) {
          Assert-ExvItemWithoutReparsePoint $child $Label
          if (($child.Attributes -band [IO.FileAttributes]::Directory) -ne 0) {
            $pending.Push([IO.DirectoryInfo]$child)
          }
        }
      }
    }
  }

  $probe = [IO.Path]::GetFullPath($Path)
  while ($null -ne $probe -and $probe.Length -gt 0) {
    $item = Get-ExvExistingItem $probe $Label
    if ($null -ne $item) {
      Assert-ExvItemWithoutReparsePoint $item $Label
    }
    $parent = Split-Path -Parent $probe
    if ([string]::IsNullOrEmpty($parent) -or (Test-ExvSamePath $parent $probe)) {
      break
    }
    $probe = $parent
  }
}

function Get-ExvSharedInstallerResidue([string]$SharedArtifactRoot) {
  if (-not (Test-Path -LiteralPath $SharedArtifactRoot -PathType Container)) {
    return @()
  }
  return @(
    Get-ChildItem -LiteralPath $SharedArtifactRoot -File -Force -ErrorAction Stop |
      Where-Object {
        $_.Name -match '^EXV-.+-windows-x64-setup\.exe$' -or
        $_.Name -match '^EXV-.+-windows-x64-setup\.exe\.exvp$' -or
        $_.Name -match '^\.EXV-.+-windows-x64-setup\.(?:staging|previous)\.exe(?:\.exvp)?$'
      }
  )
}

function Get-ExvEffectiveInstallerRetention {
  param(
    [Parameter(Mandatory = $true)]
    [string]$OutputRoot,
    [Parameter(Mandatory = $true)]
    [string]$CanonicalSharedRoot,
    [Parameter(Mandatory = $true)]
    [int]$RequestedKeep
  )

  if (Test-ExvSamePath $OutputRoot $CanonicalSharedRoot) {
    return 1
  }
  return [Math]::Min($RequestedKeep, 2)
}

function Invoke-ExvCanonicalInstallerRetention {
  param(
    [Parameter(Mandatory = $true)]
    [string[]]$Worktrees,
    [Parameter(Mandatory = $true)]
    [string]$SharedArtifactRoot,
    [switch]$WhatIf
  )

  $sharedRoot = [IO.Path]::GetFullPath($SharedArtifactRoot)
  $worktreeInstallers = @(
    foreach ($worktree in $Worktrees) {
      if (Test-Path -LiteralPath $worktree -PathType Container) {
        Get-ChildItem -LiteralPath $worktree -Recurse -File -Filter 'EXV-*-windows-x64-setup.exe' -Force -ErrorAction Stop
      }
    }
  )
  $sharedResidue = @(Get-ExvSharedInstallerResidue $sharedRoot)
  $sharedInstallers = @($sharedResidue | Where-Object { $_.Name -match '^EXV-.+-windows-x64-setup\.exe$' })
  $candidates = @($worktreeInstallers + $sharedInstallers | Where-Object { Test-ExvValidInstaller $_ })
  $selected = @($candidates | Sort-Object @{ Expression = 'LastWriteTimeUtc'; Descending = $true }, @{ Expression = 'FullName'; Descending = $false } | Select-Object -First 1)

  if ($selected.Count -eq 1) {
    $destination = Join-Path $sharedRoot $selected[0].Name
    if ($WhatIf) {
      Write-Host "Would retain canonical installer: source=$($selected[0].FullName) destination=$destination"
    } else {
      Assert-ExvNoReparsePoint $sharedRoot 'canonical shared artifact root before create'
      New-Item -ItemType Directory -Path $sharedRoot -Force -ErrorAction Stop | Out-Null
      Assert-ExvNoReparsePoint $sharedRoot 'canonical shared artifact root before copy'
      $sharedItem = Get-Item -LiteralPath $sharedRoot -Force -ErrorAction Stop
      if (($sharedItem.Attributes -band [IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Refusing to write shared artifacts through a reparse point: $sharedRoot"
      }
      if (-not (Test-ExvSamePath $selected[0].FullName $destination)) {
        Copy-Item -LiteralPath $selected[0].FullName -Destination $destination -Force -ErrorAction Stop
        $sourceHash = (Get-FileHash -LiteralPath $selected[0].FullName -Algorithm SHA256).Hash
        $destinationHash = (Get-FileHash -LiteralPath $destination -Algorithm SHA256).Hash
        if ($sourceHash -ne $destinationHash) {
          throw "Shared installer hash mismatch: $destination"
        }
      }
      $destinationItem = Get-Item -LiteralPath $destination -Force -ErrorAction Stop
      if (-not (Test-ExvValidInstaller $destinationItem)) {
        throw "Canonical installer is invalid: $destination"
      }
    }
  } else {
    Write-Host 'No valid installer exists to retain at the canonical shared root.'
  }

  $retainedPath = if ($selected.Count -eq 1) { Join-Path $sharedRoot $selected[0].Name } else { $null }
  $removals = @($worktreeInstallers + $sharedResidue | Sort-Object FullName -Unique)
  foreach ($item in $removals) {
    if ($null -ne $retainedPath -and (Test-ExvSamePath $item.FullName $retainedPath)) {
      continue
    }
    if ($WhatIf) {
      Write-Host "Would remove installer residue: $($item.FullName)"
    } elseif (Test-Path -LiteralPath $item.FullName -PathType Leaf) {
      Assert-ExvNoReparsePoint $sharedRoot 'canonical shared artifact root before remove'
      Assert-ExvNoReparsePoint $item.FullName 'installer residue before remove'
      Remove-Item -LiteralPath $item.FullName -Force -ErrorAction Stop
    }
  }
}
