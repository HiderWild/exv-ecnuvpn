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

# NRPT 规则投影（Get-DnsClientNrptRule -> name/namespaces/servers/comment）。
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
        }
    }
    , @($out)
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
    , @($out)
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
    , @($out)
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
                Write-Response @{ ok = $true; op = 'list'; rules = @(Get-NrptRuleDtos) }
            }
            'routes' {
                Write-Response @{ ok = $true; op = 'routes'; routes = @(Get-RouteDtos) }
            }
            'dnservers' {
                Write-Response @{ ok = $true; op = 'dnservers'; ifaces = @(Get-DnsInterfaceDtos) }
            }
            'nrpt_delete' {
                # 仅按规则名删除调用方判定为自有残留的规则；外来规则从不进入本 op。
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
            'bye' {
                Write-Response @{ ok = $true; op = 'bye' }
                $done = $true
            }
            default {
                Write-Response @{ ok = $false; op = $op; error = 'unknown-op' }
            }
        }
    } catch {
        $message = ''
        try { $message = [string]$_.Exception.Message } catch { }
        Write-Response @{ ok = $false; op = $op; error = $message }
    }
}
