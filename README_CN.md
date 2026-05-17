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
- **桌面客户端** — 基于 Electron 的图形界面，支持 macOS 与 Windows；无需浏览器即可管理 VPN、配置和日志
- **服务安装** — 一次性提权安装特权 helper（macOS 使用 launchd，Windows 使用系统服务）；日常使用无需 sudo/管理员权限
- **加密凭据存储** — AES-256-CBC 加密密码，密钥权限 0600
- **WebUI** — 浏览器管理界面，实时查看状态、编辑配置、查看日志（兼容选项，推荐使用桌面客户端）
- **自动重连** — 断线后自动恢复连接
- **路由自定义** — 随时添加/删除分流路由
- **VPN 服务器路由保护** — 自动防止 VPN 服务器自身流量被隧道吞没

## 安装

### 桌面客户端（推荐）

推荐通过 **桌面客户端** 使用 EXV（macOS / Windows）。它内置了原生 VPN 二进制文件，提供图形界面来连接 VPN、配置路由和查看日志——无需终端或浏览器。

1. 从 Releases 页面下载最新版本。
2. **macOS**：拖入"应用程序"文件夹后运行。首次启动时会提示输入管理员密码以安装特权 helper。
3. **Windows**：运行 NSIS 安装程序（或便携版 `.exe`）。首次启动时接受 UAC 提示以安装 `exv-helper` Windows 服务。
4. 在桌面客户端中输入学号和密码，点击连接。

### 服务安装（所有平台）

无论使用桌面客户端还是 CLI，安装特权 helper 都是一次性操作，之后无需重复 `sudo`/管理员提权：

```bash
# macOS / Linux
sudo exv service install

# Windows（以管理员身份运行）
exv.exe service install
```

安装后，`exv` 和 `exv stop` 均无需提权权限。

## 构建顺序

项目有严格的构建依赖链，必须按以下顺序执行：

1. **前端构建** — `cd webui && npm install && npm run build`
2. **原生构建** — `cmake -B build && cmake --build build`
3. **桌面构建**（可选） — `cd webui && npm run desktop:build`

前端必须先构建，因为 C++ 构建会运行 `scripts/embed_assets.py`，该脚本读取 `webui/dist/` 并生成 `src/webui_assets.hpp`。如果 `webui/dist/` 不存在，原生构建将失败。

### 前置依赖

- **openconnect** — VPN 连接核心
- **CMake** — 构建系统
- **Node.js**（v18+） — 构建前端所需（前端必须在原生二进制之前构建）
- **OpenSSL**（仅 Linux 需要） — Linux 上的 AES-256-CBC 加密库；macOS 用系统 CommonCrypto，Windows 用 CNG/BCrypt

### 完整构建

```bash
git clone <repo>
cd ECNU-VPN

# 1. 先构建前端
cd webui
npm install
npm run build
cd ..

# 2. 构建原生二进制
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build

# 3. 安装到系统路径
sudo cmake --install build    # macOS / Linux
```

### 安装 root helper（一次性 sudo）

```bash
sudo exv service install
```

`service install` 会自动将 `exv` 复制到 `/usr/local/bin/exv`（macOS/Linux）或注册 Windows 服务，并设置特权 helper 守护进程。之后日常使用不再需要 sudo/管理员权限。

## 平台支持

### macOS
```bash
brew install openconnect
cd webui && npm install && npm run build && cd ..
cmake -B build && cmake --build build
sudo exv service install
```

### Linux (Ubuntu/Debian)
```bash
sudo apt install openconnect libssl-dev cmake build-essential
cd webui && npm install && npm run build && cd ..
cmake -B build && cmake --build build
sudo ./scripts/install-linux.sh
```

### Linux (Fedora/RHEL)
```bash
sudo dnf install openconnect openssl-devel cmake gcc-c++
cd webui && npm install && npm run build && cd ..
cmake -B build && cmake --build build
sudo ./scripts/install-linux.sh
```

### Windows
1. 安装 [openconnect-gui](https://github.com/openconnect/openconnect-gui/releases)（提供 `openconnect.exe` 与 GnuTLS 运行时 DLL）。
2. 先构建前端：`cd webui && npm install && npm run build && cd ..`
3. 构建：`cmake -B build && cmake --build build --config Release`
   - Windows 端的 AES-256-CBC 已切到系统自带的 **BCrypt (CNG)**，不再需要安装 OpenSSL。
4. 以管理员身份运行：`.\build\Release\exv.exe service install`

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

两种产物携带相同的原生运行时包。运行时必须在打包前 stage 完成，否则打包后的应用连接 VPN 时会报 `runtime_missing` 错误。

#### 安装版 vs 便携版 — 行为差异

| 方面 | NSIS 安装版 | 便携版 |
|------|------------|--------|
| **安装方式** | 安装到 `Program Files`（需要管理员权限） | 单个 `.exe`，无需安装 |
| **服务注册** | 安装过程中提示注册（UAC 已激活）；执行 `exv.exe service install` | 启动时不安装服务；需手动从"服务"页面安装 |
| **UAC 提示（日常使用）** | 服务安装后无提示 | 每次连接 VPN 均需 UAC 提权，直到安装服务 |
| **运行时查找路径** | `%ProgramFiles%\ECNU VPN\bin\` | `<便携版exe所在目录>\bin\`（相对于便携版可执行文件） |
| **快捷方式** | 创建桌面和开始菜单快捷方式 | 无——直接运行 `.exe` |
| **卸载** | 卸载程序先停止并注销服务，再删除文件 | 删除 `.exe` 即可——如已安装服务需先手动卸载 |
| **应用数据** | 卸载时保留（`deleteAppDataOnUninstall: false`） | 相同——无论哪种产物，配置均存于 `%APPDATA%\ecnuvpn\` |

## 快速开始（CLI）

```bash
# 1. 设置学号
exv config set username
# > Enter value for username: 20XXXXXXXXX

# 2. 设置密码（隐匿输入，加密存储）
exv config set password
# >   New password: ••••••••
# >   Confirm password: ••••••••

# 3. 启动 VPN（安装 helper 后无需 sudo）
exv

# 4. 停止 VPN
exv stop
```

首次运行任意命令时，程序会自动创建配置文件与密钥文件：macOS/Linux 位于 `~/.ecnuvpn/`，Windows 位于 `%APPDATA%\ecnuvpn\`。

## 使用方法

### VPN 控制

| 命令 | 说明 |
|------|------|
| `exv` | 启动 VPN 并返回 shell |
| `exv stop` / `exv -s` | 停止 VPN |
| `exv status` / `exv -t` | 查看 VPN 状态与网络接口 |

### 启动选项

| 选项 | 说明 |
|------|------|
| `-rt [count]` | 断线后自动重连（详见下方） |
| `-f` / `--foreground` | 前台运行 WebUI — 兼容模式（Ctrl+C 停止） |

#### `-rt` 自动重连

默认情况下（不指定 `-rt`），VPN 断开后不会自动重连。使用 `-rt` 可启用自动重连：

```bash
exv -rt          # 无限重连，直到手动停止
exv -rt -1       # 同上，无限重连
exv -rt 3        # 最多重连 3 次
exv -rt 0        # 不重连（等同于默认行为）
```

- 仅在启动 VPN 时生效，不能与 `stop`、`status` 等命令组合使用
- 只能指定一次，重复指定会报错
- 启用后程序会 fork 一个 supervisor 进程监控 openconnect，断线时自动重新连接
- 重连次数达到上限后 supervisor 自动退出

### 配置管理

| 命令 | 说明 |
|------|------|
| `exv config` / `exv config show` | 显示当前配置（密码脱敏） |
| `exv config set <key>` | 交互式设置配置项 |
| `exv config import <file>` | 从 JSON 文件导入配置 |
| `exv config reset` | 重置为默认配置（密钥保留） |

可设置的 key：`server`、`username`、`password`、`mtu`、`useragent`、`log_file`、`webui_port`、`webui_bind`、`webui_enabled`。

### 路由管理

这是分流功能的核心。默认已内置 9 条华东师大校园网路由，你也可以自行添加：

```bash
# 查看当前所有分流路由
exv config routes list

# 添加一条路由（CIDR 格式，自动去重）
exv config routes add 10.0.0.0/8

# 删除一条路由
exv config routes remove 10.0.0.0/8
```

> **路由格式**：使用 CIDR 表示法，如 `192.168.1.0/24`（网段）或 `219.228.60.69`（单 IP）。

### WebUI（浏览器兼容模式）

浏览器 WebUI 适用于桌面客户端不可用的环境（如 Linux 或无头服务器），提供：

- 实时 VPN 状态与流量监控
- 配置在线编辑
- 实时日志流
- VPN 启停控制
- 路由管理

WebUI **不会默认启动**。要启动它，使用 `exv --webui`（启动 VPN + WebUI 服务器）或 `exv --webui --foreground`（前台模式，Ctrl+C 停止）。WebUI 默认监听地址为 `http://127.0.0.1:18080/`。

macOS 和 Windows 推荐使用桌面客户端。WebUI 是兼容/调试选项。

### Helper 服务管理

| 命令 | 说明 | 需要 sudo |
|------|------|-----------|
| `exv service install` | 安装特权 helper | 是 |
| `exv service uninstall` | 卸载特权 helper | 是 |
| `exv service status` | 查看 helper 状态 | 否 |

### VPN 模式

- **Helper 模式** — 日常使用的推荐模式。安装特权 helper 服务后（一次性 `exv service install`），桌面客户端和 CLI 都可以在无需 sudo/管理员提权的情况下启动/停止 VPN。
- **提权模式** — 当 helper 服务未安装时，桌面客户端可以使用一次性提权（Windows 的 UAC 提示，macOS 的 sudo 提示）来建立临时 VPN 会话。适用于快速使用，但不像 helper 模式那样提供持久便利。
- **直接模式** — 桌面客户端在提权授权下直接管理 VPN 的内部回退方式。当 helper 不可用且用户授予提权时自动使用。

为获得持久便利，请通过桌面客户端的服务页面或 `exv service install` 安装 helper 服务。

## 默认路由

程序默认内置了华东师范大学校内部分资源机器的路由信息，连接 VPN 后**仅这些 IP 的流量走 VPN 隧道**，其余流量走本地网络。

可通过 `exv config routes list` 查看当前路由，也可自行添加或删除。

## 注意事项

1. **首次使用需提权安装 helper** — `sudo exv service install`（macOS/Linux）或以管理员身份运行（Windows），仅需执行一次，之后 `exv` 和 `exv stop` 均无需提权。

2. **密码加密存储** — 密码使用 AES-256-CBC 加密后存入 `config.json`，密钥文件（macOS/Linux 为 `~/.ecnuvpn/.key`，Windows 为 `%APPDATA%\ecnuvpn\.key`）权限为 0600。`config show` 输出始终脱敏。

3. **路由格式必须为 CIDR** — 添加路由时使用 CIDR 表示法（如 `10.0.0.0/8`），单 IP 可省略掩码（如 `219.228.60.69`）。格式错误会导致路由不生效。

4. **VPN 服务器路由自动保护** — 程序会自动检测 VPN 服务器 IP 是否被分流路由覆盖，若覆盖则添加一条指向默认网关的主机路由，防止 VPN 连接自身被隧道吞没（"split-brain" 问题）。

5. **openconnect 必须已安装** — macOS 上若未安装，启动时会提示是否通过 Homebrew 自动安装。

6. **WebUI 默认仅监听本地** — 默认绑定 `127.0.0.1:18080`，仅本机可访问。如需局域网访问，修改 `webui_bind` 为 `0.0.0.0`（注意安全风险）。

7. **前台模式用于调试/兼容** — `exv -f` 在前台运行 WebUI（兼容模式），Ctrl+C 可停止。CLI 默认行为是启动 VPN 并返回 shell。

8. **卸载时先卸载 helper** — 卸载程序前执行 `sudo exv service uninstall`（或 Windows 等效操作）清理 helper 守护进程。

## 常见问题

**Q：连接 VPN 后上网变慢了？**

这正是本项目要解决的问题。确认你使用的是 `exv` 而非原生 AnyConnect 客户端。`exv` 通过分流路由仅让校园网流量走 VPN。运行 `exv config routes list` 确认路由配置正确。

**Q：VPN 启动失败，提示 openconnect not installed？**

程序会询问是否自动安装：`Install openconnect now? [Y/n]`，回车即可。也可手动 `brew install openconnect`。

**Q：VPN 已连接但校内资源访问不了？**

检查路由配置：`exv config routes list`，确认目标 IP 所在网段已添加。如果访问的校内 IP 不在默认路由表中，需要手动添加。

**Q：提示 "helper daemon is not installed"？**

运行 `sudo exv service install` 安装 helper。安装后 `exv` 和 `exv stop` 均无需 sudo。

**Q：密码显示 `[KEY MISSING]` 或 `[KEY CORRUPT]`？**

密钥文件损坏，需要重新生成：

```bash
exv config key reset    # 重新生成密钥（原密码密文将被清除）
exv config set password # 重新设置密码
```

**Q：如何添加新的校园网路由？**

```bash
exv config routes add 202.120.96.0/19
```

添加后下次启动 VPN 时生效。也可通过桌面客户端或 WebUI 的路由管理页面添加。

**Q：`exv stop` 提示找不到进程？**

可能 PID 文件已丢失，helper 会回退到 `pgrep` 查找。若仍失败，可手动 `sudo killall openconnect`。

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
- **Vue 3 + TypeScript + Vite** — 前端（桌面客户端与浏览器 WebUI 共用）
- **Electron** — 桌面客户端壳（macOS / Windows）
- **CommonCrypto** (macOS) / **OpenSSL** (Linux) / **CNG BCrypt** (Windows) — AES-256-CBC 加密
- **launchd** (macOS) / **Windows 服务** (Windows) / **systemd** (Linux) — 特权 helper 管理

## 许可证

[MIT](LICENSE)

## 桌面客户端开发（Electron）

EXV 的主要界面是基于 Electron 的桌面客户端，它将 Vue 前端包装在原生窗口中。桌面壳通过 Electron IPC 与原生 `exv desktop-rpc` JSON 接口通信——不依赖浏览器 WebUI 服务器。

**开发构建：**

```bash
cd webui
npm install
npm run build
npm run build:electron

# 构建原生 C++ 二进制后，打包桌面客户端
npm run desktop:build
```

实时开发模式：先构建原生二进制（或设置 `EXV_PATH` 指向已有的 `exv`/`exv.exe`），然后运行：

```bash
cd webui
npm run desktop:dev
```
