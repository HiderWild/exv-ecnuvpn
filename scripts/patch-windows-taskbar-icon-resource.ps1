param(
  [Parameter(Mandatory = $true)]
  [string]$ExePath,
  [string]$IcoPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ([string]::IsNullOrWhiteSpace($IcoPath)) {
  $repoRoot = Split-Path -Parent $PSScriptRoot
  $IcoPath = Join-Path $repoRoot 'src\platform\win32\rust\tauri\app\icons\icon.ico'
}
if (-not (Test-Path -LiteralPath $ExePath -PathType Leaf)) { throw "Missing EXE to patch: $ExePath" }
if (-not (Test-Path -LiteralPath $IcoPath -PathType Leaf)) { throw "Missing ICO to embed: $IcoPath" }

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class ExvTaskbarIconWriter {
  [DllImport("kernel32.dll", EntryPoint = "BeginUpdateResourceW", CharSet = CharSet.Unicode, SetLastError = true)]
  public static extern IntPtr BeginUpdateResource(string fileName, [MarshalAs(UnmanagedType.Bool)] bool deleteExistingResources);
  [DllImport("kernel32.dll", EntryPoint = "UpdateResourceW", SetLastError = true)]
  [return: MarshalAs(UnmanagedType.Bool)]
  public static extern bool UpdateResource(IntPtr update, IntPtr type, IntPtr name, ushort language, byte[] data, uint size);
  [DllImport("kernel32.dll", EntryPoint = "EndUpdateResourceW", SetLastError = true)]
  [return: MarshalAs(UnmanagedType.Bool)]
  public static extern bool EndUpdateResource(IntPtr update, [MarshalAs(UnmanagedType.Bool)] bool discard);
}
'@

function Get-LastWin32ErrorMessage([string]$operation) {
  $code = [Runtime.InteropServices.Marshal]::GetLastWin32Error()
  return "$operation failed (Win32=$code)"
}

$icoBytes = [IO.File]::ReadAllBytes((Resolve-Path $IcoPath).Path)
if ($icoBytes.Length -lt 6 -or [BitConverter]::ToUInt16($icoBytes, 0) -ne 0 -or [BitConverter]::ToUInt16($icoBytes, 2) -ne 1) {
  throw "ICO header is invalid: $IcoPath"
}

$frameCount = [BitConverter]::ToUInt16($icoBytes, 4)
if ($frameCount -ne 9 -or $icoBytes.Length -lt 6 + 16 * $frameCount) {
  throw "ICO must contain the nine supported taskbar frames: $IcoPath"
}

$groupBytes = New-Object byte[] (6 + 14 * $frameCount)
[Array]::Copy($icoBytes, 0, $groupBytes, 0, 6)
$iconType = 3
$groupIconType = 14
$groupIconId = 32512
$language = [UInt16]1033
$firstIconId = 101

$exe = (Resolve-Path $ExePath).Path
$update = [ExvTaskbarIconWriter]::BeginUpdateResource($exe, $false)
if ($update -eq [IntPtr]::Zero) { throw (Get-LastWin32ErrorMessage "BeginUpdateResource for $exe") }

$discard = $true
try {
  for ($index = 0; $index -lt $frameCount; $index++) {
    $iconDirectoryOffset = 6 + 16 * $index
    $payloadLength = [BitConverter]::ToUInt32($icoBytes, $iconDirectoryOffset + 8)
    $payloadOffset = [BitConverter]::ToUInt32($icoBytes, $iconDirectoryOffset + 12)
    if ($payloadLength -eq 0 -or $payloadOffset + $payloadLength -gt $icoBytes.Length) {
      throw "ICO frame $index payload is invalid: $IcoPath"
    }
    $payload = New-Object byte[] $payloadLength
    [Array]::Copy($icoBytes, [int]$payloadOffset, $payload, 0, [int]$payloadLength)
    $iconId = [UInt16]($firstIconId + $index)
    if (-not [ExvTaskbarIconWriter]::UpdateResource($update, [IntPtr]$iconType, [IntPtr]$iconId, $language, $payload, [uint32]$payload.Length)) {
      throw (Get-LastWin32ErrorMessage "UpdateResource ICON $iconId")
    }

    $groupEntryOffset = 6 + 14 * $index
    [Array]::Copy($icoBytes, $iconDirectoryOffset, $groupBytes, $groupEntryOffset, 12)
    $groupBytes[$groupEntryOffset + 12] = [byte]($iconId -band 0xff)
    $groupBytes[$groupEntryOffset + 13] = [byte](($iconId -shr 8) -band 0xff)
  }

  if (-not [ExvTaskbarIconWriter]::UpdateResource($update, [IntPtr]$groupIconType, [IntPtr]$groupIconId, $language, $groupBytes, [uint32]$groupBytes.Length)) {
    throw (Get-LastWin32ErrorMessage "UpdateResource GROUP_ICON $groupIconId")
  }
  $discard = $false
} finally {
  if (-not [ExvTaskbarIconWriter]::EndUpdateResource($update, $discard)) {
    throw (Get-LastWin32ErrorMessage 'EndUpdateResource')
  }
}

Write-Host "Patched default Windows GROUP_ICON with $frameCount native taskbar frames: $ExePath"
