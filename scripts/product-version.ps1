# Product metadata is maintained by release.py; build commands only check it.
function Get-ExvProductVersion([string]$RepositoryRoot, [string]$RequestedVersion = '') {
  $python = @('python', 'python3', 'py') | Where-Object { Get-Command $_ -ErrorAction SilentlyContinue } | Select-Object -First 1
  if (-not $python) { throw 'Python 3.9+ is required for product version validation' }
  $arguments = @()
  if ($python -eq 'py') { $arguments += '-3' }
  $arguments += @((Join-Path $RepositoryRoot 'scripts/release.py'), '--root', $RepositoryRoot, 'version', 'check', '--notes')
  if (-not [string]::IsNullOrWhiteSpace($RequestedVersion)) { $arguments += @('--expect', $RequestedVersion) }
  $result = & $python @arguments
  if ($LASTEXITCODE -ne 0) { throw 'Product version validation failed; run python scripts/release.py version check' }
  return ([string]($result | Select-Object -Last 1)).Trim()
}
