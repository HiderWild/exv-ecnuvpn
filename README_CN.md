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
- **桌面端图形界面** — Electron 桌面应用，连接/断开/配置/状态一览无余
- **加密凭据存储** — AES-256-CBC 加密密码，密钥权限 0600
- **免 sudo 日常使用** — launchd root helper，安装一次后无需再输 sudo
- **一次性授权连接** — 未安装 helper 时，通过管理员授权完成临时连接
- **自动重连** — 断线后自动恢复连接
- **路由自定义** — 随时添加/删除分流路由
- **VPN 服务器路由保护** — 自动防止 VPN 服务器自身流量被隧道吞没
- **浏览器 WebUI（兼容模式）** — 保留浏览器入口以兼容旧版

## 安装

### 前置依赖

- **openconnect** — VPN 连接核心
- **CMake** — 构建系统
- **OpenSSL**（仅 Linux 需要） — Linux 上的 AES-256-CBC 加密库；macOS 用系统 CommonCrypto，Windows 用 CNG/BCrypt

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
1. 安装 [openconnect-gui](https://github.com/openconnect/openconnect-gui/releases)（提供 `openconnect.exe` 与 GnuTLS 运行时 DLL）。
2. 构建：`cmake -B build && cmake --build build --config Release`
   - Windows 端的 AES-256-CBC 已切到系统自带的 **BCrypt (CNG)**，不再需要安装 OpenSSL。
3. 以管理员身份运行：`.\build\Release\exv.exe service install`

### Windows 桌面打包：便携版 + 安装版

Electron 桌面端可以同时产出便携版和安装版。

```powershell
# 1. 一次性 stage openconnect 运行时（DLL + wintun.dll + 可选 TAP 资源）
powershell -ExecutionPolicy Bypass -File scripts\stage-openconnect-runtime-win.ps1 -SourceDir <openconnect-gui 安装目录>

# 2. 构建 Electron 桌面包（两个目标都会输出到 webui/release/）
cd webui
npm install
npm run desktop:build
```

产物位于 `webui/release/`：

- `ECNU-VPN-<version>-portable.exe`：便携版，单文件，双击即用，不会安装服务。
- `ECNU VPN Setup <version>.exe`：NSIS 安装版，支持选择安装目录，并自动通过 `installer.nsh` 注册 `exv-helper` 服务。

打包后的 `bin/` 目录只包含必要文件：`exv.exe`、MinGW 运行时 DLL、`openconnect.exe`、GnuTLS / libxml2 / wintun.dll，以及（如果 stage 过）TAP 资源。`libssl-3-x64.dll` 与 `libcrypto-3-x64.dll` 被打包脚本主动剔除——Windows 端不再依赖 OpenSSL。

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
    "webui_enabled": false
}
```

## 技术栈

- **C++17** + CMake 构建
- **openconnect** — VPN 连接核心
- **nlohmann/json** — JSON 解析
- **cpp-httplib** — 嵌入式 HTTP 服务器
- **Vue 3 + TypeScript + Vite** — WebUI 前端
- **CommonCrypto** (macOS) / **OpenSSL** (Linux) / **CNG BCrypt** (Windows) — AES-256-CBC 加密

## 许可证

[MIT](LICENSE)

## Desktop UI (Electron)

桌面端是推荐的交互入口。Electron 壳层通过 `exv desktop-rpc` JSON 接口与 native binary 通信，不依赖浏览器 WebUI 服务器。

macOS 上，桌面端支持：
- **辅助服务连接** — 安装 launchd helper 后，连接/断开由守护进程管理
- **一次性授权连接** — 未安装 helper 时，通过 osascript 管理员授权完成临时连接
- **路由清理状态** — 断开时显示清理进度，确保不留残留路由

```bash
cd webui
npm install
npm run build
npm run build:electron
npm run desktop:build
```

开发模式：先构建 native binary 或设置 `EXV_PATH`，然后运行 `npm run desktop:dev`。

浏览器 WebUI 保留为兼容入口，可通过 `exv -f` 在前台模式启用。
