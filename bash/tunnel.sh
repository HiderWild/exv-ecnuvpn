#!/bin/bash

# =================================================================
# macOS OpenConnect 路由分流脚本 (Final Version)
# 核心逻辑：配置网卡 + 批量添加 ECNU 专用路由
# =================================================================

# 1. 拦截非连接状态
# 只有在握手成功 (reason=connect) 时才执行配置
if [ "$reason" != "connect" ]; then
    exit 0
fi

echo ">>> [VPN] 握手成功，开始配置网络..."
echo ">>> [VPN] 接口: $TUNDEV | 内网IP: $INTERNAL_IP4_ADDRESS"

# 2. 激活虚拟网卡 (必须步骤)
# 将分配到的内网 IP 绑定到 utun 接口
ifconfig "$TUNDEV" "$INTERNAL_IP4_ADDRESS" "$INTERNAL_IP4_ADDRESS" netmask 255.255.255.255 up

# 3. 定义路由表 (ECNU 专用)
# 包含了你的 SSH 目标、DNS 服务器以及各大校内网段
# 来源：你提供的 IPRANGE 变量
ECNU_ROUTES="
49.52.4.0/25
59.78.176.0/20
59.78.199.0/21
58.198.176.128/25
219.228.60.69
219.228.63.0/21
202.120.80.0/20
222.66.117.0/24
"

# 4. 批量添加路由
echo ">>> [VPN] 正在写入路由表 (Split Tunneling)..."

# 使用循环遍历上面的列表，逐条添加
for subnet in $ECNU_ROUTES; do
    # -interface $TUNDEV 强制流量走 VPN 网卡
    # 2>/dev/null 屏蔽“路由已存在”的非致命报错
    route add "$subnet" -interface "$TUNDEV" >/dev/null 2>&1
    
    if [ $? -eq 0 ]; then
        echo "  [+] 路由添加成功: $subnet"
    else
        echo "  [-] 路由添加警告: $subnet (可能已存在)"
    fi
done

echo ">>> [VPN] 网络配置完毕！"
echo ">>> [提示] 内网走 VPN，外网走 默认路由未变(如直连或Clash)。"

exit 0