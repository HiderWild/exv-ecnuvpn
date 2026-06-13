$ErrorActionPreference = 'Stop'

$repoRoot = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$startScript = Join-Path $repoRoot 'start.ps1'
$startScript = (Resolve-Path -LiteralPath $startScript).Path
& $startScript @args
exit $LASTEXITCODE
