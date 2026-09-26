param(
  [Parameter(Mandatory = $true)]
  [string]$ExePath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) {
  throw "Missing EXE to inspect: $ExePath"
}

$repoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$canonicalIcoPath = Join-Path $repoRoot 'src\platform\win32\rust\tauri\app\icons\icon.ico'
if (-not (Test-Path -LiteralPath $canonicalIcoPath -PathType Leaf)) {
  throw "Missing canonical taskbar ICO: $canonicalIcoPath"
}

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class ExvTaskbarIconResource {
  [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
  public static extern IntPtr LoadLibraryEx(string fileName, IntPtr file, uint flags);
  [DllImport("kernel32.dll", SetLastError = true)]
  [return: MarshalAs(UnmanagedType.Bool)]
  public static extern bool FreeLibrary(IntPtr module);
  [DllImport("kernel32.dll", EntryPoint = "FindResourceW", SetLastError = true)]
  public static extern IntPtr FindResource(IntPtr module, IntPtr name, IntPtr type);
  [DllImport("kernel32.dll", SetLastError = true)]
  public static extern uint SizeofResource(IntPtr module, IntPtr resource);
  [DllImport("kernel32.dll", SetLastError = true)]
  public static extern IntPtr LoadResource(IntPtr module, IntPtr resource);
  [DllImport("kernel32.dll", SetLastError = true)]
  public static extern IntPtr LockResource(IntPtr loadedResource);
}
'@

$loadLibraryAsDataFile = 0x00000002
$groupIconType = 14
$groupIconId = 32512
$module = [ExvTaskbarIconResource]::LoadLibraryEx((Resolve-Path $ExePath).Path, [IntPtr]::Zero, $loadLibraryAsDataFile)
if ($module -eq [IntPtr]::Zero) { throw "LoadLibraryEx failed for $ExePath (Win32=$([Runtime.InteropServices.Marshal]::GetLastWin32Error()))" }

function Read-IcoPayloads([string]$path) {
  $bytes = [System.IO.File]::ReadAllBytes($path)
  if ($bytes.Length -lt 6 -or [BitConverter]::ToUInt16($bytes, 0) -ne 0 -or [BitConverter]::ToUInt16($bytes, 2) -ne 1) {
    throw "Canonical ICO header is invalid: $path"
  }
  $count = [BitConverter]::ToUInt16($bytes, 4)
  if ($bytes.Length -lt 6 + 16 * $count) { throw "Canonical ICO directory is truncated: $path" }
  $frames = @()
  for ($index = 0; $index -lt $count; $index++) {
    $offset = 6 + 16 * $index
    $rawWidth = [int]$bytes[$offset]
    $rawHeight = [int]$bytes[$offset + 1]
    $width = if ($rawWidth -eq 0) { 256 } else { $rawWidth }
    $height = if ($rawHeight -eq 0) { 256 } else { $rawHeight }
    $payloadLength = [BitConverter]::ToUInt32($bytes, $offset + 8)
    $payloadOffset = [BitConverter]::ToUInt32($bytes, $offset + 12)
    if ($payloadLength -eq 0 -or $payloadOffset + $payloadLength -gt $bytes.Length) {
      throw "Canonical ICO frame payload is invalid: $path frame $index"
    }
    $payload = New-Object byte[] $payloadLength
    [Array]::Copy($bytes, [int]$payloadOffset, $payload, 0, [int]$payloadLength)
    $frames += [pscustomobject]@{
      Index = $index
      Width = $width
      Height = $height
      Payload = $payload
    }
  }
  return $frames
}

function Read-ResourceBytes([IntPtr]$module, [int]$resourceType, [UInt16]$resourceId, [string]$description) {
  $resource = [ExvTaskbarIconResource]::FindResource($module, [IntPtr]$resourceId, [IntPtr]$resourceType)
  if ($resource -eq [IntPtr]::Zero) { throw "$description is absent from $ExePath" }
  $size = [ExvTaskbarIconResource]::SizeofResource($module, $resource)
  $loaded = [ExvTaskbarIconResource]::LoadResource($module, $resource)
  $pointer = [ExvTaskbarIconResource]::LockResource($loaded)
  if ($size -eq 0 -or $loaded -eq [IntPtr]::Zero -or $pointer -eq [IntPtr]::Zero) {
    throw "$description is malformed in $ExePath"
  }
  $bytes = New-Object byte[] $size
  [Runtime.InteropServices.Marshal]::Copy($pointer, $bytes, 0, $bytes.Length)
  return $bytes
}

function Assert-BytesEqual([byte[]]$actual, [byte[]]$expected, [string]$description) {
  if ($actual.Length -ne $expected.Length) {
    throw "$description byte length differs from canonical ICO: actual=$($actual.Length), expected=$($expected.Length)"
  }
  for ($index = 0; $index -lt $expected.Length; $index++) {
    if ($actual[$index] -ne $expected[$index]) {
      throw "$description differs from canonical ICO at byte $index"
    }
  }
}

try {
  $bytes = Read-ResourceBytes $module $groupIconType ([UInt16]$groupIconId) "GROUP_ICON resource $groupIconId"
  if ($bytes.Length -lt 6) { throw "GROUP_ICON resource is malformed: $ExePath" }
  $count = [BitConverter]::ToUInt16($bytes, 4)
  if ($bytes.Length -ne (6 + 14 * $count)) { throw "GROUP_ICON entry count is malformed: $ExePath" }

  $expected = @(16, 20, 24, 32, 40, 48, 64, 128, 256)
  $actual = @()
  $canonicalFrames = @(Read-IcoPayloads $canonicalIcoPath)
  if ($canonicalFrames.Count -ne $expected.Count) { throw "Canonical ICO must contain $($expected.Count) frames: $canonicalIcoPath" }
  for ($index = 0; $index -lt $count; $index++) {
    $offset = 6 + 14 * $index
    $rawWidth = [int]$bytes[$offset]
    $rawHeight = [int]$bytes[$offset + 1]
    $width = if ($rawWidth -eq 0) { 256 } else { $rawWidth }
    $height = if ($rawHeight -eq 0) { 256 } else { $rawHeight }
    if ($width -ne $height) { throw "GROUP_ICON has a non-square frame: $ExePath entry $index" }
    if ($width -eq 256) {
      if ($rawWidth -ne 0 -or $rawHeight -ne 0) { throw "256px GROUP_ICON frame must use zero dimensions: $ExePath entry $index" }
    } elseif ($rawWidth -ne $width -or $rawHeight -ne $height) {
      throw "GROUP_ICON directory dimensions are incorrect: $ExePath entry $index"
    }
    if ([BitConverter]::ToUInt16($bytes, $offset + 4) -ne 1 -or [BitConverter]::ToUInt16($bytes, $offset + 6) -ne 32) {
      throw "GROUP_ICON frame must be 32bpp: $ExePath entry $index"
    }
    $canonical = @($canonicalFrames | Where-Object { $_.Width -eq $width -and $_.Height -eq $height })
    if ($canonical.Count -ne 1) { throw "Canonical ICO must contain exactly one ${width}px frame: $canonicalIcoPath" }
    $iconId = [BitConverter]::ToUInt16($bytes, $offset + 12)
    if ($iconId -eq 0) { throw "GROUP_ICON has an invalid RT_ICON id: $ExePath entry $index" }
    $payload = Read-ResourceBytes $module 3 $iconId "RT_ICON $iconId for ${width}px GROUP_ICON entry"
    Assert-BytesEqual $payload $canonical[0].Payload "RT_ICON $iconId for ${width}px GROUP_ICON entry"
    $actual += $width
  }
  if (@(Compare-Object $expected ($actual | Sort-Object -Unique)).Count -ne 0) {
    throw "EXE GROUP_ICON size set must be exactly $($expected -join ', '): $ExePath has $($actual -join ', ')"
  }
  Write-Host 'PASS packaged EXE embeds every native taskbar icon frame byte-identical to the canonical ICO, including 16px, 20px, 40px, and 48px'
} finally {
  [void][ExvTaskbarIconResource]::FreeLibrary($module)
}
