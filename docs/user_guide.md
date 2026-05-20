# EXV 使用手册

> 包装 openconnect 的智能 VPN 客户端，支持分流路由、加密凭据管理、launchd root helper 与 WebUI。
>
> 当前版本：**v3.3.0** | macOS | 需要 openconnect | 推荐安装一次 helper 后日常免 sudo

---

## 安装

### 构建

```bash
git clone <repo>
cd ECNU-VPN
cmake --preset macos-release
cmake --build --preset macos-release
```

### 安装到系统路径

```bash
sudo cmake --install build/macos/cpp
```

### 构建目录约定

为减少 `windows` / `macos` 分支在合并前的产物冲突，新的构建输出固定分叉到：

- `build/windows/cpp`
- `build/windows/electron/*`
- `build/macos/cpp`
- `build/macos/electron/*`

推荐直接使用平台脚本：

```bash
# macOS
./scripts/build-macos.sh all

# Windows
powershell -ExecutionPolicy Bypass -File scripts\build-windows.ps1 -Action all
```

### 安装 root helper（一次性 sudo）

```bash
sudo exv service install
```

`service install` 会自动将 `exv` 复制到 `/usr/local/bin/exv` 并注册 launchd 守护进程。之后日常使用不再需要 sudo。

---

## 快速开始

### 首次使用

首次运行任意命令时，程序自动完成初始化：
- 创建 `~/.ecnuvpn/config.json`（默认配置）
- 生成 AES-256 加密密钥 → `~/.ecnuvpn/.key`（权限 0600）

```bash
# 1. 设置学号
exv config set username
# > Enter value for username: 20XXXXXXXXX

# 2. 设置密码（隐匿输入，不回显）
exv config set password
# >   New password: ••••••••
# >   Confirm password: ••••••••

# 3. 安装一次 helper（仅首次需要 sudo）
sudo exv service install

# 4. 之后日常直接启动（无需 sudo）
exv
```

---

## 命令参考

```
exv [command] [subcommand] [args]
```

### VPN 控制

| 命令 | 说明 | 需要 sudo |
|------|------|-----------|
| `exv` | 启动 VPN（含 WebUI） | 安装 helper 后 ❌ |
| `exv stop` \| `-s` | 停止 VPN | 安装 helper 后 ❌ |
| `exv status` \| `-t` | 查看 VPN 状态与网络接口 | ❌ |

### 启动选项

| 选项 | 说明 |
|------|------|
| `-rt [count]` | 断线后自动重连次数（省略或 -1 为无限） |
| `-f`, `--foreground` | 前台运行 WebUI（Ctrl+C 停止） |

### Helper 服务管理

| 命令 | 说明 | 需要 sudo |
|------|------|-----------|
| `exv service install` | 安装 launchd root helper | ✅ |
| `exv service uninstall` | 卸载 launchd root helper | ✅ |
| `exv service status` | 查看 helper 状态 | ❌ |

### 配置管理

| 命令 | 说明 |
|------|------|
| `exv config` \| `config show` | 显示当前配置（密码脱敏） |
| `exv config set <key>` | 交互式设置配置项 |
| `exv config import <file>` | 从 JSON 文件导入配置 |
| `exv config reset` | 重置为默认配置（密钥保留） |

**可设置的 key：**

| Key | 说明 |
|-----|------|
| `server` | VPN 服务器地址 |
| `username` | 登录学号 |
| `password` | 登录密码（隐匿输入，加密存储） |
| `mtu` | MTU 值（默认 1290） |
| `useragent` | User-Agent 字符串 |
| `log_file` | 日志文件路径 |
| `webui_port` | WebUI 端口（默认 18080） |
| `webui_bind` | WebUI 绑定地址（默认 127.0.0.1） |
| `webui_enabled` | 是否启用 WebUI（默认 true） |

### 路由管理

| 命令 | 说明 |
|------|------|
| `exv config routes list` | 列出所有分流路由 |
| `exv config routes add <cidr>` | 添加路由（自动去重） |
| `exv config routes remove <cidr>` | 删除路由 |

### 密钥管理

| 命令 | 说明 |
|------|------|
| `exv config key show` | 查看密钥文件路径与有效性 |
| `exv config key reset` | 重新生成密钥（清除密码密文，需确认） |

### 日志与帮助

| 命令 | 说明 |
|------|------|
| `exv logs` \| `-l` | 查看最近 50 条日志 |
| `exv help` \| `-h` | 帮助信息 |
| `exv version` \| `-v` | 版本号 |

---

## WebUI

VPN 启动后自动在后台运行 WebUI（默认 `http://127.0.0.1:18080/`），提供：

- 实时 VPN 状态与流量监控
- 配置在线编辑
- 实时日志流（SSE）
- VPN 启停控制

WebUI 默认启用，可通过 `exv config set webui_enabled` 设为 `false` 关闭。

---

## 配置文件格式

配置文件位于 `~/.ecnuvpn/config.json`，可手动编辑（密码字段存储的是密文）：

```json
{
    "server": "https://vpn-cn.ecnu.edu.cn",
    "username": "20XXXXXXXXX",
    "password": "<AES-256-CBC ciphertext>",
    "mtu": 1290,
    "useragent": "AnyConnect Darwin_x86_64 4.10.05095",
    "routes": [
        "49.52.4.0/25",
        "59.78.176.0/20"
    ],
    "extra_args": [],
    "log_file": "~/.ecnuvpn/ecnuvpn.log",
    "webui_port": 18080,
    "webui_bind": "127.0.0.1",
    "webui_enabled": true
}
```

**通过 `config import` 导入时**，`password` 字段可写明文——程序会自动加密后存储。

---

## 加密说明

- 密码通过 **AES-256-CBC** 加密（macOS 内置 CommonCrypto，无需额外依赖）
- 密钥（32 字节随机数）存储于 `~/.ecnuvpn/.key`，文件权限 **0600**
- `config show` 始终展示脱敏结果（`••••••••  (encrypted)`）

### 密钥故障处理

如果 `config show` 显示 `[KEY MISSING]` 或 `[KEY CORRUPT]`：

```bash
exv config key reset    # 重新生成密钥（原密码密文将被清除）
exv config set password # 重新设置密码
```

---

## 运行时文件

| 文件 | 说明 |
|------|------|
| `~/.ecnuvpn/config.json` | 配置（密码为密文） |
| `~/.ecnuvpn/.key` | AES-256 加密密钥（0600） |
| `~/.ecnuvpn/tunnel.sh` | 隧道脚本（每次 start 自动生成） |
| `~/.ecnuvpn/ecnuvpn.pid` | openconnect 进程 PID |
| `~/.ecnuvpn/ecnuvpn-supervisor.pid` | 自动重连 supervisor PID |
| `~/.ecnuvpn/route-ready` | 路由配置完成标记（接口名 + 内网 IP） |
| `~/.ecnuvpn/ecnuvpn.log` | 带时间戳的运行日志 |

### 系统级文件

| 文件 | 说明 |
|------|------|
| `/Library/LaunchDaemons/com.ecnu.exv.helper.plist` | launchd root helper 定义 |
| `/var/run/exv-helper.sock` | 本地 Unix socket（root:staff 0660），普通用户通过它请求 root helper |
| `/var/run/exv-helper-session.json` | 当前 helper 管理的会话状态 |

---

## 常见问题

**Q：VPN 启动失败，提示 openconnect not installed？**
程序会询问是否自动安装：`Install openconnect now? [Y/n]`，回车即可通过 Homebrew 安装。

**Q：VPN 已连接但校内资源访问不了？**
检查路由配置：`exv config routes list`，确认目标 IP 所在网段已添加。

**Q：为什么不用每次 `sudo exv` 了？**
`sudo exv service install` 安装 launchd root helper 后，`exv` / `exv stop` 通过 Unix socket（`/var/run/exv-helper.sock`，group staff）请求 helper 代为执行特权操作。

**Q：`exv` 提示 "helper daemon is not installed"？**
运行 `sudo exv service install` 安装 helper。安装后 `exv` 和 `exv stop` 均无需 sudo。

**Q：`exv stop` 提示找不到进程？**
可能 PID 文件已丢失，helper 会回退到 `pgrep` 查找。若 helper 未安装，可先执行 `sudo exv service install`；仍失败时再手动 `sudo killall openconnect`。
