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

- **macOS**（本项目仅支持 macOS）
- **openconnect** — VPN 连接核心

```bash
# 安装 openconnect（如果尚未安装）
brew install openconnect
```

### 构建与安装

```bash
git clone <repo>
cd ECNU-VPN
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(sysctl -n hw.ncpu)

# 安装到系统路径
sudo cmake --install build
```

### 安装 root helper（一次性）

```bash
sudo exv service install
```

此命令会将 `exv` 复制到 `/usr/local/bin/exv` 并注册 launchd 守护进程。**安装后日常使用不再需要 sudo。**

## Platform Support

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
1. Install [openconnect-gui](https://github.com/openconnect/openconnect-gui/releases)
2. Install OpenSSL (via [vcpkg](https://vcpkg.io/) or [choco](https://chocolatey.org/)): `choco install openssl`
3. Build: `cmake -B build && cmake --build build --config Release`
4. Run as Administrator: `.\build\Release\exv.exe install-helper`

## 快速开始

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

首次运行任意命令时，程序会自动创建 `~/.ecnuvpn/config.json`（默认配置）和 `~/.ecnuvpn/.key`（加密密钥）。

## 使用方法

### VPN 控制

| 命令 | 说明 |
|------|------|
| `exv` | 启动 VPN（含 WebUI） |
| `exv stop` / `exv -s` | 停止 VPN |
| `exv status` / `exv -t` | 查看 VPN 状态与网络接口 |

### 启动选项

| 选项 | 说明 |
|------|------|
| `-rt [count]` | 断线后自动重连（详见下方） |
| `-f` / `--foreground` | 前台运行 WebUI（Ctrl+C 停止） |

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

### WebUI

VPN 启动后自动在后台运行 WebUI，默认地址 `http://127.0.0.1:18080/`，提供：

- 实时 VPN 状态与流量监控
- 配置在线编辑
- 实时日志流
- VPN 启停控制
- 路由管理

关闭 WebUI：`exv config set webui_enabled` 设为 `false`。

### Helper 服务管理

| 命令 | 说明 | 需要 sudo |
|------|------|-----------|
| `exv service install` | 安装 launchd root helper | 是 |
| `exv service uninstall` | 卸载 launchd root helper | 是 |
| `exv service status` | 查看 helper 状态 | 否 |

## 默认路由

程序默认内置了华东师范大学校内部分资源机器的路由信息，连接 VPN 后**仅这些 IP 的流量走 VPN 隧道**，其余流量走本地网络。

可通过 `exv config routes list` 查看当前路由，也可自行添加或删除。

## 注意事项

1. **首次使用需 sudo 安装 helper** — `sudo exv service install` 仅需执行一次，之后 `exv` 和 `exv stop` 均无需 sudo。

2. **密码加密存储** — 密码使用 AES-256-CBC 加密后存入 `config.json`，密钥文件 `~/.ecnuvpn/.key` 权限为 0600。`config show` 输出始终脱敏。

3. **路由格式必须为 CIDR** — 添加路由时使用 CIDR 表示法（如 `10.0.0.0/8`），单 IP 可省略掩码（如 `219.228.60.69`）。格式错误会导致路由不生效。

4. **VPN 服务器路由自动保护** — 程序会自动检测 VPN 服务器 IP 是否被分流路由覆盖，若覆盖则添加一条指向默认网关的主机路由，防止 VPN 连接自身被隧道吞没（"split-brain" 问题）。

5. **openconnect 必须已安装** — 若未安装，启动时会提示是否通过 Homebrew 自动安装。

6. **WebUI 默认仅监听本地** — 默认绑定 `127.0.0.1:18080`，仅本机可访问。如需局域网访问，修改 `webui_bind` 为 `0.0.0.0`（注意安全风险）。

7. **前台模式用于调试** — `exv -f` 在前台运行 WebUI，Ctrl+C 可停止。默认为后台模式。

8. **卸载时先卸载 helper** — 卸载程序前执行 `sudo exv service uninstall` 清理 launchd 守护进程。

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

添加后下次启动 VPN 时生效。也可通过 WebUI 的路由管理页面添加。

**Q：`exv stop` 提示找不到进程？**

可能 PID 文件已丢失，helper 会回退到 `pgrep` 查找。若仍失败，可手动 `sudo killall openconnect`。

## 配置文件

配置文件位于 `~/.ecnuvpn/config.json`：

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

通过 `config import` 导入时，`password` 字段可写明文，程序会自动加密后存储。

## 技术栈

- **C++17** + CMake 构建
- **openconnect** — VPN 连接核心
- **nlohmann/json** — JSON 解析
- **cpp-httplib** — 嵌入式 HTTP 服务器
- **CommonCrypto** — macOS 内置加密（AES-256-CBC）
- **Vue 3 + TypeScript + Vite** — WebUI 前端
- **launchd** — macOS 服务管理

## 许可证

[MIT](LICENSE)
