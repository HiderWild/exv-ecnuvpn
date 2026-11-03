# 预检常驻 PowerShell 子进程（2026-09-27 计划 §三 v6 底质）。
# 一次 spawn、顺序承载各域查询与清理 op；stdin EOF 或 {"op":"bye"} 即退出。
# 协议：一行 JSON 请求 -> 一行 JSON 应答（请求-应答按序配对，无流式终结歧义）。
# 就绪通告：启动后先输出一行 {"ready":true}。
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

function Write-Protocol([string]$Text) {
    try { [Console]::Out.WriteLine($Text); [Console]::Out.Flush() } catch { }
}

function Write-Response($Payload) {
    try { Write-Protocol ($Payload | ConvertTo-Json -Compress -Depth 6) } catch { }
}

# NRPT 规则投影（Get-DnsClientNrptRule -> name/namespaces/servers/comment/标志位）。
# 标志位是等效接纳的排除条件（DNSSEC/IPsec/DirectAccess 任一启用即不等效）；
# 只读投影，属性缺失按 $false（旧形态规则/极端环境不因此误判等效——Rust 侧
# 同一指纹规则消费，见 preflight/classify.rs）。
function Get-NrptRuleDtos {
    $out = @()
    foreach ($rule in @(Get-DnsClientNrptRule)) {
        $comment = ''
        if ($null -ne $rule.Comment) { $comment = [string]$rule.Comment }
        $out += [ordered]@{
            name = [string]$rule.Name
            namespaces = @($rule.Namespace | ForEach-Object { [string]$_ })
            servers = @($rule.NameServers | ForEach-Object { [string]$_ })
            comment = $comment
            dnssec = [bool]$rule.DnsSecValidationRequired
            ipsec = [bool]$rule.DnsSecQueryIPsecRequired
            direct_access = [bool]$rule.DirectAccessEnabled
        }
    }
    $out
}

# 有效策略表投影（Get-DnsClientNrptPolicy -Effective -> namespaces/servers/标志位）。
# 只读。外来规则可能只在有效表存在（2026-10-08 事故形态：本地表 0 条）——
# 本投影使预检/认领清理器/等效判定都能看见它。策略对象无 Name/Comment 字段，
# 也没有 IPsec 投影属性（DnsClientPolicyConfiguration 形态所限）；Rust 侧指纹
# 规则对有效表条目按「可观测标志」判定并在注释中如实说明。
function Get-NrptEffectiveDtos {
    $out = @()
    foreach ($policy in @(Get-DnsClientNrptPolicy -Effective)) {
        $out += [ordered]@{
            namespaces = @($policy.Namespace | ForEach-Object { [string]$_ })
            servers = @($policy.NameServers | ForEach-Object { [string]$_ })
            dnssec = [bool]$policy.DnsSecValidationRequired
            direct_access = [bool]$policy.DirectAccessEnabled
        }
    }
    $out
}

# IPv4 路由投影（Get-NetRoute -> dest/prefix/next_hop/ifindex/metric/protocol）。
# metric 取 RouteMetric（路由分量；engine 安装 bypass 行时固定写 1）。
function Get-RouteDtos {
    $out = @()
    foreach ($route in @(Get-NetRoute -AddressFamily IPv4)) {
        $parts = ([string]$route.DestinationPrefix) -split '/'
        $prefix = 0
        if ($parts.Count -ge 2) { $prefix = [int]$parts[1] }
        $metric = 0
        if ($null -ne $route.RouteMetric) { $metric = [int]$route.RouteMetric }
        $out += [ordered]@{
            dest = $parts[0]
            prefix = $prefix
            next_hop = [string]$route.NextHop
            ifindex = [int]$route.InterfaceIndex
            metric = $metric
            protocol = [string]$route.Protocol
        }
    }
    $out
}

# 接口 DNS 投影（Get-DnsClientServerAddress -> ifindex/alias/servers）。
function Get-DnsInterfaceDtos {
    $out = @()
    foreach ($iface in @(Get-DnsClientServerAddress -AddressFamily IPv4)) {
        $out += [ordered]@{
            ifindex = [int]$iface.InterfaceIndex
            alias = [string]$iface.InterfaceAlias
            servers = @($iface.ServerAddresses | ForEach-Object { [string]$_ })
        }
    }
    $out
}

Write-Protocol '{"ready":true}'

$done = $false
while (-not $done -and $null -ne ($line = [Console]::In.ReadLine())) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    $request = $null
    try { $request = $line | ConvertFrom-Json } catch { }
    $op = ''
    if ($null -ne $request -and $null -ne $request.op) { $op = [string]$request.op }
    try {
        switch ($op) {
            'list' {
                # 本地规则表 + 有效策略表一次返回（外来规则可能只在有效表）。
                Write-Response @{ ok = $true; op = 'list'; rules = @(Get-NrptRuleDtos); effective = @(Get-NrptEffectiveDtos) }
            }
            'routes' {
                Write-Response @{ ok = $true; op = 'routes'; routes = @(Get-RouteDtos) }
            }
            'dnservers' {
                Write-Response @{ ok = $true; op = 'dnservers'; ifaces = @(Get-DnsInterfaceDtos) }
            }
            'nrpt_delete' {
                # 按规则名删除。两类调用方（D8 阶段 2 起）：① 预检自有前缀（EXV-NRPT:）
                # 残留清理；② 静默精准接管的 rule_table 分支——只删 takeover_targets
                # 判定的外来冲突规则（journal 前置完成后才调用）。
                $name = [string]$request.name
                if ([string]::IsNullOrWhiteSpace($name)) { throw 'nrpt_delete: name' }
                Remove-DnsClientNrptRule -Name $name -Force -Confirm:$false -ErrorAction Stop
                Write-Response @{ ok = $true; op = 'nrpt_delete' }
            }
            'route_delete' {
                # 仅删除死接口（dangling）路由；-Confirm:$false 免交互。
                $dest = [string]$request.dest
                $prefixText = $dest + '/' + ([int]$request.prefix)
                $ifindex = [int]$request.ifindex
                $hop = [string]$request.next_hop
                if ([string]::IsNullOrEmpty($hop) -or $hop -eq '0.0.0.0') {
                    Remove-NetRoute -DestinationPrefix $prefixText -InterfaceIndex $ifindex -Confirm:$false -ErrorAction Stop
                } else {
                    Remove-NetRoute -DestinationPrefix $prefixText -InterfaceIndex $ifindex -NextHop $hop -Confirm:$false -ErrorAction Stop
                }
                Write-Response @{ ok = $true; op = 'route_delete' }
            }
            'flushdns' {
                Clear-DnsClientCache -ErrorAction Stop
                Write-Response @{ ok = $true; op = 'flushdns' }
            }
            'restart_dnscache' {
                # 仅 D8 静默精准接管的 effective_only 分支调用（仅有效表覆盖无公开
                # 寻址手段，重启清空 Dnscache 内存态规则）。护栏：NRPT worker 未启动、
                # install_live 之前、无线上隧道；全系统解析中断 1-3 秒（风险已声明）。
                # Rust 侧用专用 8s 超时等待本 op（Restart-Service 负载下可超 2s——
                # 原操作超时直接返回，调用者结束本次接管）。
                Restart-Service Dnscache -Force -ErrorAction Stop
                Write-Response @{ ok = $true; op = 'restart_dnscache' }
            }
            'bye' {
                Write-Response @{ ok = $true; op = 'bye' }
                $done = $true
            }
            default {
                Write-Response @{ ok = $false; op = $op; error = 'unknown-op' }
            }
        }
    } catch {
        $record = $_
        $message = ''
        try { $message = [string]$record.Exception.Message } catch { }
        # 当前接管动作沿原应答携带异常，其他预检域继续原 error 文本合同。
        if ($op -in @('list', 'nrpt_delete', 'restart_dnscache', 'flushdns')) {
            $exception = $record.Exception
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
            $message = [string]$exception.Message
            Write-Response @{ ok = $false; op = $op; error = $message; native_code = $nativeCode; hresult = [long]$exception.HResult; exception_type = $exception.GetType().FullName; error_id = $record.FullyQualifiedErrorId }
        } else {
            Write-Response @{ ok = $false; op = $op; error = $message }
        }
    }
}
