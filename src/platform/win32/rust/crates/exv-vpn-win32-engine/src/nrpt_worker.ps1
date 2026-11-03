# 长驻、无窗口的 NRPT 规则 owner。stdin EOF 是唯一正常释放信号。
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$script:owner = 'EXV-NRPT:' + [Guid]::NewGuid().ToString('D')
$script:namespaces = @()
$script:servers = @()
$script:mayHaveRule = $false
$script:stage = 'input'
$script:readySent = $false
$script:failed = $false

function Write-Protocol([string]$Text) {
    try { [Console]::Out.WriteLine($Text); [Console]::Out.Flush() } catch { }
}

function Test-ExactOwnedRule($Rule, [string]$Namespace) {
    $actualNamespaces = @($Rule.Namespace | ForEach-Object { [string]$_ })
    $actualServers = @($Rule.NameServers | ForEach-Object { [string]$_ })
    return $Rule.Comment -ceq $script:owner -and
        $actualNamespaces.Count -eq 1 -and $actualNamespaces[0] -ceq $Namespace -and
        (Compare-Object -ReferenceObject $script:servers -DifferenceObject $actualServers -CaseSensitive).Count -eq 0
}

try {
    $line = [Console]::In.ReadLine()
    if ($null -eq $line) { throw 'protocol' }
    $request = $line | ConvertFrom-Json -ErrorAction Stop
    $domains = @($request.domains | ForEach-Object { [string]$_ })
    $script:servers = @($request.servers | ForEach-Object { [string]$_ })
    if ($domains.Count -lt 1 -or $script:servers.Count -lt 1) { throw 'input' }
    foreach ($domain in $domains) {
        if ($domain.Length -gt 253 -or $domain -match '^[0-9.]+$' -or $domain -notmatch '^(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?)(?:\.(?:[A-Za-z0-9](?:[A-Za-z0-9-]{0,61}[A-Za-z0-9])?))*$') { throw 'input' }
        $script:namespaces += $domain.ToLowerInvariant()
        $script:namespaces += ('.' + $domain.ToLowerInvariant())
    }
    foreach ($server in $script:servers) {
        $parsed = $null
        if (-not [Net.IPAddress]::TryParse($server, [ref]$parsed)) { throw 'input' }
    }
    if (@($script:namespaces | Select-Object -Unique).Count -ne $script:namespaces.Count -or
        @($script:servers | Select-Object -Unique).Count -ne $script:servers.Count) { throw 'input' }

    $script:stage = 'conflict'
    $configured = @(Get-DnsClientNrptRule)
    $effective = @(Get-DnsClientNrptPolicy -Effective)
    foreach ($namespace in $script:namespaces) {
        $localConflict = @($configured | Where-Object { @($_.Namespace | ForEach-Object {[string]$_}) -ccontains $namespace })
        $effectiveConflict = @($effective | Where-Object { @($_.Namespace | ForEach-Object {[string]$_}) -ccontains $namespace })
        if ($localConflict.Count -gt 0 -or $effectiveConflict.Count -gt 0) { throw 'conflict' }
    }

    $script:stage = 'add'
    foreach ($namespace in $script:namespaces) {
        # Add 前登记恢复义务；超时/异常后仍按本次 owner GUID 注释寻找部分写入。
        $script:mayHaveRule = $true
        Add-DnsClientNrptRule -Namespace $namespace -NameServers $script:servers -Comment $script:owner -DisplayName $script:owner -PassThru | Out-Null
    }

    $script:stage = 'verify'
    $owned = @()
    foreach ($namespace in $script:namespaces) {
        $matches = @(Get-DnsClientNrptRule | Where-Object { Test-ExactOwnedRule $_ $namespace })
        if ($matches.Count -ne 1) { throw 'verify' }
        $owned += $matches
    }
    $effective = @(Get-DnsClientNrptPolicy -Effective)
    foreach ($namespace in $script:namespaces) {
        $matches = @($effective | Where-Object {
            @($_.Namespace | ForEach-Object {[string]$_}) -ccontains $namespace -and
            (Compare-Object -ReferenceObject $script:servers -DifferenceObject @($_.NameServers | ForEach-Object {[string]$_}) -CaseSensitive).Count -eq 0
        })
        if ($matches.Count -lt 1) { throw 'verify' }
    }
    Clear-DnsClientCache
    # ready 应答回带本次 owner 标记（EXV-NRPT:<guid>）：engine 侧记入连接痕迹
    # （T4 trace 的 nrpt_owner_guids；预检 NRPT 域按同一标记认领自有孤儿规则）。
    Write-Protocol ('{"ready":true,"owner":"' + $script:owner + '"}')
    $script:readySent = $true
    # 持有 stdin 管道即持有规则；Engine 崩溃会令此循环读到 EOF。
    while ($null -ne [Console]::In.ReadLine()) { }
} catch {
    $script:failed = $true
    $kind = if ($script:stage -eq 'conflict') { 'conflict' } elseif ($script:stage -eq 'input') { 'input' } elseif ($script:stage -eq 'verify') { 'verify' } else { 'apply' }
    if (-not $script:readySent) { Write-Protocol ('{"ready":false,"error":"' + $kind + '"}') }
} finally {
    # 删除失败时继续持有自己的清理义务；Engine 的有界等待不强杀这个清理进程。
    do {
        $restored = $true
        if ($script:mayHaveRule) {
            try {
                $rules = @(Get-DnsClientNrptRule | Where-Object { $_.Comment -ceq $script:owner })
                foreach ($rule in $rules) {
                    $ruleNamespaces = @($rule.Namespace | ForEach-Object {[string]$_})
                    # 其他程序已经改变的规则不再删除，沿用比较恢复语义。
                    if ($ruleNamespaces.Count -ne 1 -or $ruleNamespaces[0] -cnotin $script:namespaces) { continue }
                    if (-not (Test-ExactOwnedRule $rule $ruleNamespaces[0])) { continue }
                    try { Remove-DnsClientNrptRule -Name $rule.Name -Force -Confirm:$false }
                    catch { $restored = $false }
                }
                $remaining = @(Get-DnsClientNrptRule | Where-Object {
                    $names = @($_.Namespace)
                    $_.Comment -ceq $script:owner -and $names.Count -eq 1 -and
                    $names[0] -cin $script:namespaces -and (Test-ExactOwnedRule $_ $names[0])
                })
                if ($remaining.Count -gt 0) { $restored = $false }
                Clear-DnsClientCache
            } catch { $restored = $false }
        }
        if (-not $restored) { Write-Protocol '{"cleanup":"incomplete"}'; Start-Sleep -Seconds 1 }
    } while (-not $restored)
    Write-Protocol '{"cleanup":"complete"}'
}
