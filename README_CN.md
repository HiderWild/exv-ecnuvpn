# EXV — 华东师范大学智能 VPN 客户端

> 解决 Cisco AnyConnect 全局路由导致网络卡慢的问题，仅让校园网流量走 VPN，其余流量走本地网络。

## 问题背景

使用 Cisco AnyConnect 连接校园 VPN 后，**默认路由会将所有流量都发送到 VPN 隧道**。这意味着：

- 访问百度、B站、微信等国内资源时，数据包绕经校园 VPN 再出去，延迟高、速度慢
- 访问国外网站同样绕远路，体验极差
- 校园网带宽有限，所有流量挤在一起互相抢占

**EXV 的解决方案**：通过 **分流路由（Split Tunneling）**，仅将校园网 IP 段的流量路由到 VPN 隧道，其他流量继续走本地默认路由。连接 VPN 后，日常上网速度不受影响，同时可以正常访问校内资源。

## 功能特性

- **分流路由** — 仅校园网流量走 VPN，其余走本地网络
- **加密凭据存储** — AES-256-CBC 加密密码，密钥权限 0600
- **免 sudo 日常使用** — launchd root helper，安装一次后无需再输 sudo
- **WebUI 管理界面** — 浏览器实时查看状态、编辑配置、查看日志
- **自动重连** — 断线后自动恢复连接
- **路由自定义** — 随时添加/删除分流路由
- **VPN 服务器路由保护** — 自动防止 VPN 服务器自身流量被隧道吞没

## 安装

### 前置依赖

- **openconnect** — VPN 连接核心
- **CMake** — 构建系统
- **OpenSSL** — 加密库

### 构建与安装

```bash
git clone <repo>
cd ECNU-VPN
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build
```

## 平台支持

### macOS
```bash
brew install openconnect
cmake -B build && cmake --build build
sudo ./cminst.sh
```

### Linux (Ubuntu/Debian)
```bash
sudo apt install openconnect libssl-dev cmake build-essential
cmake -B build && cmake --build build
sudo ./scripts/install-linux.sh
```

### Linux (Fedora/RHEL)
```bash
sudo dnf install openconnect openssl-devel cmake gcc-c++
cmake -B build && cmake --build build
sudo ./scripts/install-linux.sh
```

### Windows
1. 安装 [openconnect-gui](https://github.com/openconnect/openconnect-gui/releases)
2. 安装 OpenSSL（通过 [vcpkg](https://vcpkg.io/) 或 [choco](https://chocolatey.org/)）：`choco install openssl`
3. 构建：`cmake -B build && cmake --build build --config Release`
4. 以管理员身份运行：`.\build\Release\exv.exe service install`

## 配置

配置文件默认位于：macOS/Linux 为 `~/.ecnuvpn/config.json`，Windows 为 `%APPDATA%\ecnuvpn\config.json`。

```json
{
    "server": "https://vpn-ct.ecnu.edu.cn",
    "username": "20XXXXXXXXX",
    "password": "<AES-256-CBC 密文>",
    "mtu": 1290,
    "useragent": "AnyConnect Darwin_x86_64 4.10.05095",
    "routes": ["49.52.4.0/25", "..."],
    "extra_args": [],
    "log_file": "~/.ecnuvpn/ecnuvpn.log",
    "webui_port": 18080,
    "webui_bind": "127.0.0.1",
    "webui_enabled": true
}
```

## 技术栈

- **C++17** + CMake 构建
- **openconnect** — VPN 连接核心
- **nlohmann/json** — JSON 解析
- **cpp-httplib** — 嵌入式 HTTP 服务器
- **Vue 3 + TypeScript + Vite** — WebUI 前端
- **CommonCrypto** (macOS) / **OpenSSL** (Linux/Windows) — AES-256-CBC 加密

## 许可证

[MIT](LICENSE)
