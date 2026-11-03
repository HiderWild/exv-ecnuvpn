#requires -Version 7.0
# 只使用合成 DPAPI 材料；ValidateOnly 不启动节点程序、不读取真实认证。
$ErrorActionPreference = 'Stop'
$repository = [IO.Path]::GetFullPath((Join-Path $PSScriptRoot ('../' * 9)))
$launcher = Join-Path $repository 'scripts/atrust-latency/invoke-node.ps1'
$temporary = Join-Path ([IO.Path]::GetTempPath()) ('exv-latency-node-test-' + [guid]::NewGuid().ToString('N'))
$null = New-Item -ItemType Directory -Path $temporary
$cache = Join-Path $temporary 'session.dpapi'
$secret = 'synthetic-private-cookie-marker'
$plain = $null
try {
    $payload = @{version=2;kind='atrust-browser-session';gateway='https://vpn.example.edu';cookie_header=('sid='+$secret);user_agent='synthetic-agent'} | ConvertTo-Json -Compress
    $plain = [Text.Encoding]::UTF8.GetBytes($payload)
    [IO.File]::WriteAllBytes($cache,[Security.Cryptography.ProtectedData]::Protect($plain,$null,[Security.Cryptography.DataProtectionScope]::CurrentUser))
    $before = (Get-FileHash -LiteralPath $cache).Hash
    $result = & $launcher -SessionCachePath $cache -ExpectedGateway 'https://vpn.example.edu' -PhysicalIfIndex 4 -ValidateOnly
    if ($result.Validated -ne $true -or $result.ServerValidity -ne 'not-checked' -or $result.CredentialTransport -ne 'stdin') { throw 'validation-summary' }
    if (($result | ConvertTo-Json -Compress).Contains($secret)) { throw 'secret-in-summary' }
    if ((Get-FileHash -LiteralPath $cache).Hash -ne $before) { throw 'cache-mutated' }
    $rejected = $false
    try { & $launcher -SessionCachePath $cache -ExpectedGateway 'https://other.example.edu' -PhysicalIfIndex 4 -ValidateOnly } catch {
        if ($_.ToString().Contains($secret)) { throw 'secret-in-error' }
        $rejected = $true
    }
    if (-not $rejected) { throw 'gateway-mismatch-accepted' }
    $rejected = $false
    try { & $launcher -SessionCachePath $cache -ExpectedGateway 'https://vpn.example.edu' -PhysicalIfIndex 0 -ValidateOnly } catch { $rejected = $true }
    if (-not $rejected) { throw 'zero-interface-accepted' }
    $result = & $launcher -SessionCachePath $cache -ExpectedGateway 'https://vpn.example.edu' -PhysicalIfIndex 4 -LabArgumentsJson '["--samples","1"]' -ValidateOnly
    if (-not $result.Validated) { throw 'json-argument-array-rejected' }
    foreach ($json in @('{}','[1]','null','"--samples"')) {
        $rejected = $false
        try { & $launcher -SessionCachePath $cache -ExpectedGateway 'https://vpn.example.edu' -PhysicalIfIndex 4 -LabArgumentsJson $json -ValidateOnly } catch { $rejected = $true }
        if (-not $rejected) { throw 'non-string-array-accepted' }
    }
    $rejected = $false
    try { & $launcher -SessionCachePath $cache -ExpectedGateway 'https://vpn.example.edu' -PhysicalIfIndex 4 -LabArguments @('--samples','1') -LabArgumentsJson '[]' -ValidateOnly } catch { $rejected = $true }
    if (-not $rejected) { throw 'two-argument-formats-accepted' }
    Write-Output 'PASS: 合成 DPAPI 校验、同源约束、秘密不输出、缓存未改写、接口参数边界'
} finally {
    if ($plain) { [Security.Cryptography.CryptographicOperations]::ZeroMemory($plain) }
    # 只删除本测试创建并已核验位于临时目录的两个确切路径。
    if (Test-Path -LiteralPath $cache) { Remove-Item -LiteralPath $cache }
    if (Test-Path -LiteralPath $temporary) { Remove-Item -LiteralPath $temporary }
    $payload = $null
}
