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

- **原生 VPN 引擎** — 生产默认运行时，不依赖 OpenConnect
- **CMake** — 构建系统
- **OpenSSL**（仅 Linux 需要） — Linux 上的 AES-256-CBC 加密库；macOS 用系统 CommonCrypto，Windows 用 CNG/BCrypt
- **OpenConnect**（可选） — 仅用于开发/诊断时对比旧后端行为，不是生产依赖

### 构建与安装

```bash
git clone <repo>
cd ECNU-VPN
cmake --preset macos-release
cmake --build --preset macos-release
```

## 构建产物目录

为了让 `windows` 与 `macos` 两条分支在合并前尽量少互相踩到产物，新的编译和打包输出都统一分叉到平台专用目录。

- `build-windows/cpp`：Windows 原生 CMake 产物、测试与 `compile_commands.json`
- `build/windows/electron/dist`：Windows renderer 产物
- `build/windows/electron/dist-electron`：Windows Electron main/preload 产物
- `build/windows/electron/native/bin`：Windows 桌面打包用原生运行时资产
- `build/windows/electron/release`：Windows 安装包与 portable 输出
- `build/macos/cpp`：macOS 原生 CMake 产物、测试与 `compile_commands.json`
- `build/macos/electron/dist`：macOS renderer 产物
- `build/macos/electron/dist-electron`：macOS Electron main/preload 产物
- `build/macos/electron/native/bin`：macOS 桌面打包用原生运行时资产
- `build/macos/electron/release`：macOS 桌面打包输出

推荐直接使用 `scripts/` 里的平台脚本，不要再默认把产物写回旧的共享目录。

## 平台支持

### macOS
```bash
./scripts/build-macos.sh all
sudo ./cminst.sh
```

macOS 原生生产包不要求安装 Homebrew OpenConnect。

只做原生编译时可以直接用 preset：

```bash
cmake --preset macos-release
cmake --build --preset macos-release
ctest --preset macos-release -R 'platform_status_models_test|vpn_runtime_test'
```

### Linux (Ubuntu/Debian)
```bash
sudo apt install libssl-dev cmake build-essential
cmake --preset linux-release
cmake --build --preset linux-release
sudo ./scripts/install-linux.sh
```

### Linux (Fedora/RHEL)
```bash
sudo dnf install openssl-devel cmake gcc-c++
cmake --preset linux-release
cmake --build --preset linux-release
sudo ./scripts/install-linux.sh
```

### Windows
1. 构建：`powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action all`
   - Windows 端的 AES-256-CBC 已切到系统自带的 **BCrypt (CNG)**，不再需要安装 OpenSSL。
   - 原生模式是生产默认，不需要安装 openconnect-gui。
2. 以管理员身份运行：`.\build-windows\cpp\exv.exe service install`

只做原生编译时可以直接用 preset：

```powershell
cmake --preset windows-release
cmake --build --preset windows-release --target exv exv-helper platform_status_models_test vpn_runtime_test
ctest --preset windows-release -R 'platform_status_models_test|vpn_runtime_test'
```

### Windows 桌面打包：便携版 + 安装版

Electron 桌面端可以同时产出便携版和安装版。

```powershell
# 构建并打包 Windows 桌面端
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action desktop
```

产物位于 `build/windows/electron/release/`：

- `ECNU-VPN-<version>-portable.exe`：便携版，单文件，双击即用，不会安装服务。
- `ECNU VPN Setup <version>.exe`：NSIS 安装版，支持选择安装目录，并自动通过 `installer.nsh` 注册 `exv-helper` 服务。

打包后的 `bin/` 目录只包含生产必要文件：`exv.exe`、`exv-helper.exe`、MinGW 运行时 DLL 与 `wintun.dll`。`wintun.dll` 是 Windows 原生模式的运行时资产；生产包不再要求暂存旧版 OpenConnect 运行时。`libssl-3-x64.dll` 与 `libcrypto-3-x64.dll` 被打包脚本主动剔除——Windows 端不再依赖 OpenSSL。

### 桌面调试构建（unpacked）

如果只需要可调试的桌面构建，而不希望直接生成安装包、便携版或 DMG，可使用下面两条平台脚本：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action debug
```

```bash
./scripts/build-macos.sh debug
```

如果希望在构建完成后自动弹出 Electron UI，可使用：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action debug-run
```

```bash
./scripts/build-macos.sh debug-run
```

unpacked Electron 目录会输出到 `build/windows/electron/release/win-unpacked/`
以及 `build/macos/electron/release/mac*/`。

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
    "log_file": "~/.ecnuvpn/ecnuvpn.log",
    "webui_port": 18080,
    "webui_bind": "127.0.0.1",
    "webui_enabled": false
}
```

原生生产模式不支持任意 `extra_args` 透传；请使用 EXV 支持的显式配置项和路由命令。旧 OpenConnect 后端只保留为开发/诊断回退。

## 技术栈

- **C++17** + CMake 构建
- **原生 VPN 协议实现** — 生产默认 VPN 连接核心
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
./scripts/build-macos.sh all
./scripts/build-macos.sh desktop
```

开发模式：先构建 native binary 或设置 `EXV_PATH`，然后运行 `pnpm run desktop:dev`。

浏览器 WebUI 保留为兼容入口，可通过 `exv -f` 在前台模式启用。

完整构建矩阵与脚本说明见 [docs/build_guide.md](docs/build_guide.md)。
