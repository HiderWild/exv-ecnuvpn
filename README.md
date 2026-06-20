# EXV - 华东师范大学智能 VPN 客户端

EXV 用于解决校园 VPN 连接后默认路由接管全局流量的问题。它通过分流路由只让校内网段走 VPN，其余流量继续走本地网络，从而保持日常访问速度并兼顾校内资源访问。

## 主要功能

- 分流路由：仅校园网流量进入 VPN 隧道。
- native WebView 桌面界面：默认桌面壳为 C++ `exv-ui`，复用系统 WebView 内核。
- 原生 VPN 引擎：生产默认运行时，不依赖 OpenConnect。
- 特权 helper：负责服务维护、网卡、DNS、路由和清理等特权操作。
- 加密凭据存储：按平台使用系统加密能力或 OpenSSL。
- 自动重连与状态事件：core 维护业务状态，桌面界面通过稳定 RPC 契约交互。

## 快速构建

### Windows

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 desktop
```

Windows 桌面包输出到：

```text
build\windows\webview\package\EXV
```

包内关键文件：

- `exv-ui.exe`：native WebView shell。
- `WebView2Loader.dll`：WebView2 SDK loader。
- `bin\exv.exe`：core 进程。
- `bin\exv-helper.exe`：特权 helper。
- `webui\index.html`：打包后的 Vue renderer。

### macOS

```bash
./scripts/build-macos.sh desktop
```

macOS 桌面包输出到：

```text
build/macos/webview/package/EXV
```

### Linux

```bash
./scripts/build-linux.sh desktop
```

Linux 桌面包输出到：

```text
build/linux/webview/package/EXV
```

## 开发入口

- Windows 本地一键重建和启动：`.\start.ps1`
- 只查看当前构建状态：`.\start.ps1 -Status`
- 只生成包不启动：`.\start.ps1 -Package`
- 跳过启动：`.\start.ps1 -NoLaunch`

`start.ps1` 使用 native WebView package 路径，不再把桌面调试流程绑定到旧桌面壳。

## 构建产物目录

- `build-windows/cpp`：Windows CMake 产物和测试。
- `build/windows/webview/dist`：Windows renderer 产物。
- `build/windows/webview/package/EXV`：Windows native WebView 桌面包。
- `build/macos/cpp`：macOS CMake 产物和测试。
- `build/macos/webview/dist`：macOS renderer 产物。
- `build/macos/webview/package/EXV`：macOS native WebView 桌面包。
- `build/linux/cpp`：Linux CMake 产物和测试。
- `build/linux/webview/dist`：Linux renderer 产物。
- `build/linux/webview/package/EXV`：Linux native WebView 桌面包。

## 运行时依赖

- Windows：WebView2 Evergreen Runtime。缺失时 native shell 会按受控策略提示安装。
- macOS：系统 WKWebView。
- Linux：WebKitGTK。
- Linux 加密依赖 OpenSSL；macOS 使用 CommonCrypto；Windows 使用 CNG/BCrypt。

本地可选运行时资产说明见 [docs/runtime-assets.md](docs/runtime-assets.md)。根目录 `runtime/` 是未跟踪的本地暂存目录，不属于仓库内容。

## 配置

配置文件默认位置：

- Windows：`%LOCALAPPDATA%\EXV\profile\default\config.json`
- macOS：`~/Library/Application Support/EXV/profile/default/config.json`
- Linux：`~/.exv/config.json`

示例：

```json
{
  "server": "https://vpn-ct.ecnu.edu.cn",
  "username": "20XXXXXXXXX",
  "password": "<encrypted>",
  "mtu": 1290,
  "routes": ["49.52.4.0/25"],
  "webui_enabled": false
}
```

## 技术栈

- C++20 + CMake 3.28+
- Vue 3 + TypeScript + Vite
- native WebView shell: WebView2 / WKWebView / WebKitGTK
- nlohmann/json
- cpp-httplib

完整构建矩阵见 [docs/build_guide.md](docs/build_guide.md)，使用说明见 [docs/user_guide.md](docs/user_guide.md)。

## 许可证

[MIT](LICENSE)
