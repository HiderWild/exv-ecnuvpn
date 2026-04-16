# ECNU-VPN 使用手册

> 包装 Cisco AnyConnect / openconnect 的智能 VPN 客户端，支持分流路由与加密凭据管理。
>
> 当前版本：**v1.0.0** | macOS | 需要 openconnect

---

## 安装

### 构建

```bash
git clone <repo>
cd ECNU-VPN
mkdir -p build && cd build
cmake .. -DCMAKE_BUILD_TYPE=Release
make -j$(sysctl -n hw.ncpu)
```

### 安装到系统路径

```bash
sudo cmake --install .
# 或者使用 cminst
cminst build/ecnuvpn -c
```

---

## 快速开始

### 首次使用

首次运行任意命令时，程序自动完成初始化：
- 创建 `~/.ecnuvpn/config.json`（默认配置）
- 生成 AES-256 加密密钥 → `~/.ecnuvpn/.key`（权限 0600）

```bash
# 1. 设置学号
ecnuvpn config set username
# > Enter value for username: 20XXXXXXXXX

# 2. 设置密码（隐匿输入，不回显）
ecnuvpn config set password
# >   New password: ••••••••
# >   Confirm password: ••••••••

# 3. 启动 VPN
sudo ecnuvpn
```

---

## 命令参考

```
ecnuvpn [command] [subcommand] [args]
```

### VPN 控制

| 命令 | 说明 | 需要 sudo |
|------|------|-----------|
| `ecnuvpn` | 启动 VPN | ✅ |
| `ecnuvpn stop` \| `-s` | 停止 VPN | ✅ |
| `ecnuvpn status` \| `-t` | 查看 VPN 状态与网络接口 | ❌ |

### 配置管理

| 命令 | 说明 |
|------|------|
| `ecnuvpn config` \| `config show` | 显示当前配置（密码脱敏） |
| `ecnuvpn config set <key>` | 交互式设置配置项 |
| `ecnuvpn config import <file>` | 从 JSON 文件导入配置 |
| `ecnuvpn config reset` | 重置为默认配置（密钥保留） |

**可设置的 key：**

| Key | 说明 |
|-----|------|
| `server` | VPN 服务器地址 |
| `username` | 登录学号 |
| `password` | 登录密码（隐匿输入，加密存储） |
| `mtu` | MTU 值（默认 1290） |
| `useragent` | User-Agent 字符串 |
| `log_file` | 日志文件路径 |

### 路由管理

| 命令 | 说明 |
|------|------|
| `ecnuvpn config routes list` | 列出所有分流路由 |
| `ecnuvpn config routes add <cidr>` | 添加路由（自动去重） |
| `ecnuvpn config routes remove <cidr>` | 删除路由 |

### 密钥管理

| 命令 | 说明 |
|------|------|
| `ecnuvpn config key show` | 查看密钥文件路径与有效性 |
| `ecnuvpn config key reset` | 重新生成密钥（清除密码密文，需确认） |

### 日志与帮助

| 命令 | 说明 |
|------|------|
| `ecnuvpn logs` \| `-l` | 查看最近 50 条日志 |
| `ecnuvpn help` \| `-h` | 帮助信息 |
| `ecnuvpn version` \| `-v` | 版本号 |

---

## 配置文件格式

配置文件位于 `~/.ecnuvpn/config.json`，可手动编辑（密码字段存储的是密文）：

```json
{
    "server": "https://vpn-ct.ecnu.edu.cn",
    "username": "20XXXXXXXXX",
    "password": "<AES-256-CBC ciphertext>",
    "mtu": 1290,
    "useragent": "AnyConnect Darwin_x86_64 4.10.05095",
    "routes": [
        "49.52.4.0/25",
        "59.78.176.0/20"
    ],
    "extra_args": [],
    "log_file": "~/.ecnuvpn/ecnuvpn.log"
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
# 重新生成密钥（原密码密文将被清除）
ecnuvpn config key reset

# 重新设置密码
ecnuvpn config set password
```

---

## 运行时文件

| 文件 | 说明 |
|------|------|
| `~/.ecnuvpn/config.json` | 配置（密码为密文） |
| `~/.ecnuvpn/.key` | AES-256 加密密钥（0600） |
| `~/.ecnuvpn/tunnel.sh` | 隧道脚本（每次 start 自动生成） |
| `~/.ecnuvpn/ecnuvpn.pid` | openconnect 进程 PID |
| `~/.ecnuvpn/ecnuvpn.log` | 带时间戳的运行日志 |

---

## 常见问题

**Q：VPN 启动失败，提示 openconnect not installed？**
程序会询问是否自动安装：`Install openconnect now? [Y/n]`，回车即可通过 Homebrew 安装。

**Q：VPN 已连接但校内资源访问不了？**
检查路由配置：`ecnuvpn config routes list`，确认目标 IP 所在网段已添加。

**Q：`sudo ecnuvpn stop` 提示找不到进程？**
可能 PID 文件已丢失，程序会回退到 `pgrep` 查找。若仍失败：`sudo killall openconnect`。
