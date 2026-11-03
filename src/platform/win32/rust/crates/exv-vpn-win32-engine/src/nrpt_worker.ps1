# 长驻、无窗口的 NRPT 规则 owner。stdin EOF 是唯一正常释放信号。
# 等效接纳（2026-10-08 计划阶段 1）：请求可携带 accept_forms——由 Rust 侧
# （preflight/classify.rs 的 nrpt_equivalence_decision）判定为「外来规则等效
# 提供」的命名空间形式清单。脚本对这些形式**跳过自有添加**（不 Add、不写 owner
# 标记），conflict 检查改查「存在且服务器集合一致」。
#
# 防两表漂移约束：等效判定的指纹规则（带点/不带点展开、大小写、序无关集合
# 相等、DNSSEC/IPsec/DirectAccess 排除、空服务器排除）**单一实现于 Rust 侧**；
# 本脚本只做字面执行——accept_forms 是 Rust 规范化后的小写字面量，脚本对它们
# 仅复用自身既有的 Compare-Object 服务器集合比较，
# 不做任何独立的「等效」裁定。
#
# 防死锁约束（2026-09-26 孤儿事故复盘）：接纳的外来规则不属于本 owner——
# finally 清理只删 Comment 等于本次 owner GUID 的规则，绝不动外来规则、
# 也绝不期待它消失；规则不在=清理成功（幂等）。
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$script:owner = 'EXV-NRPT:' + [Guid]::NewGuid().ToString('D')
$script:namespaces = @()
$script:servers = @()
$script:accepted = @()
$script:mayHaveRule = $false
$script:stage = 'input'
$script:readySent = $false
$script:failed = $false

function Write-Protocol([string]$Text) {
    try { [Console]::Out.WriteLine($Text); [Console]::Out.Flush() } catch { }
}

# 失败应答沿现有协议增加可选字段，保留实际动作、异常类型、原码和原文。
function Get-OperationFailure($Record, [string]$Operation) {
    $exception = $Record.Exception
    $nativeCode = $null
    $cause = $exception
    while ($null -ne $cause) {
        $exception = $cause
        if ($null -ne $cause.PSObject.Properties['NativeErrorCode']) {
            $nativeCode = $cause.NativeErrorCode
            break
        }
        $cause = $cause.InnerException
    }
    return @{ operation = $Operation; message = [string]$exception.Message; native_code = $nativeCode; hresult = [long]$exception.HResult; exception_type = $exception.GetType().FullName; error_id = [string]$Record.FullyQualifiedErrorId }
}

function Test-ExactOwnedRule($Rule, [string]$Namespace) {
    $actualNamespaces = @($Rule.Namespace | ForEach-Object { [string]$_ })
    $actualServers = @($Rule.NameServers | ForEach-Object { [string]$_ })
    return $Rule.Comment -ceq $script:owner -and
        $actualNamespaces.Count -eq 1 -and $actualNamespaces[0] -ceq $Namespace -and
        (Compare-Object -ReferenceObject $script:servers -DifferenceObject $actualServers -CaseSensitive).Count -eq 0
}

# 快照中的某命名空间是否存在「服务器集合与本次配置一致」的有效策略覆盖
# （仅用于操作前冲突选择；判定权威在 Rust 侧）。
function Test-EffectiveEquivalent($EffectiveSnapshot, [string]$Namespace) {
    foreach ($policy in $EffectiveSnapshot) {
        $namespaces = @($policy.Namespace | ForEach-Object { [string]$_ })
        if ($namespaces -ccontains $Namespace) {
            $policyServers = @($policy.NameServers | ForEach-Object { [string]$_ })
            if ((Compare-Object -ReferenceObject $script:servers -DifferenceObject $policyServers -CaseSensitive).Count -eq 0) {
                return $true
            }
        }
    }
    return $false
}

try {
    $line = [Console]::In.ReadLine()
    if ($null -eq $line) { throw 'protocol' }
    $request = $line | ConvertFrom-Json -ErrorAction Stop
    $domains = @($request.domains | ForEach-Object { [string]$_ })
    $script:servers = @($request.servers | ForEach-Object { [string]$_ })
    # accept_forms 缺省（旧引擎 wire 不带该字段）须得 @()：$null | ForEach-Object 会
# 流转一个 null 项（PS 管道语义），不守卫会把缺省变成 @("") 并误报 input。
$script:accepted = @()
if ($null -ne $request.accept_forms) {
    $script:accepted = @($request.accept_forms | ForEach-Object { [string]$_ })
}
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
    # accept_forms 必须是本次展开形式集的子集（Rust 侧同一 expand_campus_forms
    # 规则产出的字面量；越界即协议错误，防陈旧判定误入）。
    if (@($script:accepted | Select-Object -Unique).Count -ne $script:accepted.Count) { throw 'input' }
    foreach ($accepted in $script:accepted) {
        if ($script:namespaces -cnotcontains $accepted) { throw 'input' }
    }

    $script:stage = 'read_conflicts'
    $configured = @(Get-DnsClientNrptRule)
    $effective = @(Get-DnsClientNrptPolicy -Effective)
    # 冲突检测分级：accept 集内的形式改查「存在且等效」（Rust 判定的字面执行）；
    # 探测与启动之间的竞态按安全方向收敛——规则消失 → 转正常添加（EXV 自己提供）；
    # 规则仍在但服务器不一致 → 真冲突（原失败路径）。集外形式维持「必须不存在」。
    $script:stage = 'conflict'
    $script:skipAdd = @()
    foreach ($namespace in $script:namespaces) {
        $localConflict = @($configured | Where-Object { @($_.Namespace | ForEach-Object {[string]$_}) -ccontains $namespace })
        $effectiveConflict = @($effective | Where-Object { @($_.Namespace | ForEach-Object {[string]$_}) -ccontains $namespace })
        if ($script:accepted -ccontains $namespace) {
            if ($localConflict.Count -gt 0 -or $effectiveConflict.Count -gt 0) {
                if (Test-EffectiveEquivalent $effective $namespace) {
                    # 外来等效仍在：跳过自有添加（external-provided）。
                    $script:skipAdd += $namespace
                } else {
                    # 覆盖仍在但不再等效（服务器漂移）→ 真冲突。
                    throw 'conflict'
                }
            }
            # 覆盖已消失（竞态）→ 不跳过，转正常添加路径。
        } else {
            if ($localConflict.Count -gt 0 -or $effectiveConflict.Count -gt 0) { throw 'conflict' }
        }
    }

    $script:stage = 'add'
    foreach ($namespace in $script:namespaces) {
        if ($script:skipAdd -ccontains $namespace) { continue }
        # Add 前登记恢复义务；超时/异常后仍按本次 owner GUID 注释寻找部分写入。
        $script:mayHaveRule = $true
        Add-DnsClientNrptRule -Namespace $namespace -NameServers $script:servers -Comment $script:owner -DisplayName $script:owner -PassThru | Out-Null
    }

    $script:stage = 'flushdns'
    Clear-DnsClientCache
    # ready 应答回带本次 owner 标记（EXV-NRPT:<guid>）：engine 侧记入连接痕迹
    # （T4 trace 的 nrpt_owner_guids；预检 NRPT 域按同一标记认领自有孤儿规则）。
    # accept_forms 中的形式没有自有 owner 规则——ready 不携带它们的 owner。
    Write-Protocol ('{"ready":true,"owner":"' + $script:owner + '"}')
    $script:readySent = $true
    # 持有 stdin 管道即持有规则；Engine 崩溃会令此循环读到 EOF。
    while ($null -ne [Console]::In.ReadLine()) { }
} catch {
    $script:failed = $true
    $failure = Get-OperationFailure $_ $script:stage
    $kind = if ($script:stage -eq 'conflict') { 'conflict' } elseif ($script:stage -eq 'input') { 'input' } else { 'apply' }
    if (-not $script:readySent) { Write-Protocol (@{ ready = $false; error = $kind; failure = $failure } | ConvertTo-Json -Compress -Depth 4) }
} finally {
    # 删除失败时继续持有自己的清理义务；Engine 的有界等待不强杀这个清理进程。
    # 等效接纳的外来形式永不进入本清理（无 owner 标记即无清理义务，也不期待
    # 它消失——teardown 幂等：自有规则不在=成功）。
    do {
        $restored = $true
        $cleanupOperation = 'cleanup_select'
        if ($script:mayHaveRule) {
            try {
                $rules = @(Get-DnsClientNrptRule | Where-Object { $_.Comment -ceq $script:owner })
                foreach ($rule in $rules) {
                    $ruleNamespaces = @($rule.Namespace | ForEach-Object {[string]$_})
                    # 其他程序已经改变的规则不再删除，沿用比较恢复语义。
                    if ($ruleNamespaces.Count -ne 1 -or $ruleNamespaces[0] -cnotin $script:namespaces) { continue }
                    if (-not (Test-ExactOwnedRule $rule $ruleNamespaces[0])) { continue }
                    try { Remove-DnsClientNrptRule -Name $rule.Name -Force -Confirm:$false }
                    catch {
                        $restored = $false
                        $failure = Get-OperationFailure $_ 'remove'
                        Write-Protocol (@{ cleanup = 'incomplete'; failure = $failure } | ConvertTo-Json -Compress -Depth 4)
                    }
                }
                $cleanupOperation = 'cleanup_flushdns'
                Clear-DnsClientCache
            } catch {
                $restored = $false
                $failure = Get-OperationFailure $_ $cleanupOperation
                Write-Protocol (@{ cleanup = 'incomplete'; failure = $failure } | ConvertTo-Json -Compress -Depth 4)
            }
        }
        if (-not $restored) { Start-Sleep -Seconds 1 }
    } while (-not $restored)
    Write-Protocol '{"cleanup":"complete"}'
}
