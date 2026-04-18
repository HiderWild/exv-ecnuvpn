# ECNU-VPN 代码文档

> 面向开发者的项目结构与 API 参考。

---

## 项目结构

```
ECNU-VPN/
├── CMakeLists.txt          # C++17 构建，注入版本号
├── .gitignore
├── bash/                   # 原始 bash 脚本（保留参考）
├── docs/
│   ├── user_guide.md       # 用户使用手册
│   └── code_guide.md       # 本文档
├── include/
│   └── nlohmann/json.hpp   # Header-only JSON 库 v3.11.3
└── src/
    ├── main.cpp            # CLI 入口
    ├── config.hpp/cpp      # 配置管理
    ├── crypto.hpp/cpp      # AES-256-CBC 加密 + 密钥管理
    ├── helper.hpp/cpp      # launchd root helper / IPC / service 管理
    ├── vpn.hpp/cpp         # VPN 控制
    ├── tunnel.hpp/cpp      # tunnel.sh 动态生成
    ├── logger.hpp/cpp      # 日志系统
    └── utils.hpp/cpp       # 工具函数
```

---

## 构建系统

### CMakeLists.txt

- C++17 标准
- 通过 `target_compile_definitions` 注入 `ECNUVPN_VERSION` 宏
- 源文件：`src/*.cpp`（8 个文件，含 helper）
- 依赖：`include/nlohmann/json.hpp`（header-only）
- 系统依赖：`CommonCrypto`（macOS 内置，无需链接额外 framework）

```bash
mkdir build && cd build
cmake .. -DCMAKE_BUILD_TYPE=Release
make -j$(sysctl -n hw.ncpu)
sudo cmake --install .          # 安装到 /usr/local/bin
sudo exv service install        # 安装 launchd root helper
```

---

## 模块 API

### crypto — 加密与密钥管理

**文件**：`src/crypto.hpp` / `src/crypto.cpp`

#### 密钥管理

```cpp
// 生成 32 字节随机密钥，返回 64 位 hex string
std::string crypto::generate_key();

// 从 ~/.ecnuvpn/.key 读取；不存在返回 ""
std::string crypto::load_key();

// 写入密钥文件，设置权限 0600
bool crypto::save_key(const std::string& hex_key);

// 验证：必须为 64 个合法 hex 字符
bool crypto::validate_key(const std::string& hex_key);

// 若密钥不存在则生成并保存，返回有效 key
std::string crypto::init_key_if_needed();

// 重新生成密钥（交互确认）+ 清除 config.json 中的密码密文
bool crypto::reset_key();

// 返回 "valid" | "missing" | "corrupt"
std::string crypto::key_status();

// 返回 ~/.ecnuvpn/.key
std::string crypto::key_path();
```

#### 加密 / 解密

**算法**：AES-256-CBC，由 macOS `CommonCrypto` 提供

**密文格式**：`base64(IV[16 bytes] + AES_CBC_PKCS7_ciphertext)`

```cpp
// 加密明文，返回 base64 密文；失败返回 ""
std::string crypto::encrypt(const std::string& plaintext,
                            const std::string& hex_key);

// 解密 base64 密文，返回明文；失败返回 ""
std::string crypto::decrypt(const std::string& ciphertext_b64,
                            const std::string& hex_key);
```

#### 隐匿输入

```cpp
// 使用 termios 禁用 echo，从 stdin 读取密码后恢复
std::string crypto::read_password_hidden(const std::string& prompt);
```

**实现细节**：
- `tcgetattr` / `tcsetattr` 控制 terminal echo
- 输入后自动打印换行符（因 echo 关闭导致 Enter 不输出换行）

---

### config — 配置管理

**文件**：`src/config.hpp` / `src/config.cpp`

#### Config 结构体

```cpp
struct Config {
    std::string server;      // VPN 服务器 URL
    std::string username;
    std::string password;    // ⚠️ 存储 AES-256-CBC 密文（非明文）
    int         mtu;
    std::string useragent;
    std::vector<std::string> routes;
    std::vector<std::string> extra_args;
    std::string log_file;
};
```

使用 `NLOHMANN_DEFINE_TYPE_INTRUSIVE_WITH_DEFAULT` 宏自动实现 JSON 序列化。

#### API

```cpp
// 加载配置，首次运行自动生成密钥 + 默认 config
Config config::load();

// 序列化为 JSON 并写入磁盘
bool config::save(const Config& cfg);

// 格式化显示，密码根据密钥状态显示 ••••••• 或 [KEY CORRUPT]
void config::show(const Config& cfg);

// 解密并返回明文密码（密钥损坏时打印错误并返回 ""）
std::string config::get_plaintext_password(const Config& cfg);

// 合并导入 JSON 文件（plaintext password 自动加密后存储）
Config config::import_from(const std::string& path);

// 交互式设置配置项；password 触发隐匿双重输入 + 加密
bool config::set_value(Config& cfg, const std::string& key,
                       const std::string& value = "");

// 重置到默认值（密钥文件不受影响）
Config config::reset();

// 路由管理（持久化到 config.json）
bool config::add_route(Config& cfg, const std::string& route);
bool config::remove_route(Config& cfg, const std::string& route);
void config::list_routes(const Config& cfg);

// 密钥子命令
void config::key_show();     // 显示密钥状态
bool config::key_reset();    // 调用 crypto::reset_key()
```

#### 首次运行初始化

`config::load()` 在配置文件不存在时执行 `first_run_init()`：
1. `utils::ensure_dir(~/.ecnuvpn)`
2. `crypto::init_key_if_needed()` — 生成并保存密钥
3. 创建并保存默认 `Config`

---

### vpn — VPN 控制

**文件**：`src/vpn.hpp` / `src/vpn.cpp`

```cpp
int vpn::start(const Config& cfg, int retry_limit = 0);
int vpn::start_with_password(const Config& cfg,
                             const std::string& plaintext_password,
                             int retry_limit = 0);
int vpn::stop();
int vpn::status();
```

#### `start()` 流程

1. **openconnect 检查**：未安装则提示安装（Homebrew 流程）
2. **配置验证**：username / password / server
3. **密码解密**：`config::get_plaintext_password(cfg)`
4. **非 root 时优先走 helper**：若 `/var/run/exv-helper.sock` 可用，则通过本地 Unix socket 请求 root helper
5. **root 直连或 helper worker**：进入 `start_with_password()`
6. **生成 tunnel.sh**：`tunnel::write_script(cfg)`
7. **构建命令**：`echo '<password>' | openconnect ... --script tunnel.sh`
8. **可选 supervisor**：`-rt` 非 0 时 fork 一个重连监督进程
9. **执行 + 验证**：轮询 PID 文件和 `route-ready` 标记文件

#### PID 管理

- `~/.ecnuvpn/ecnuvpn.pid` 存储 openconnect PID
- `~/.ecnuvpn/ecnuvpn-supervisor.pid` 存储自动重连 supervisor PID
- `~/.ecnuvpn/route-ready` 由 tunnel script 在网络配置完成后写入
- `stop()` 优先读 PID 文件，失败时 fallback 到 `pgrep`
- 先发 SIGTERM（等 1.5s），再发 SIGKILL（等 1s）

---

### helper — launchd root helper 与 IPC

**文件**：`src/helper.hpp` / `src/helper.cpp`

```cpp
bool helper::is_available();
bool helper::start_via_helper(const Config& cfg,
                              const std::string& plaintext_password,
                              int retry_limit);
bool helper::stop_via_helper();
bool helper::show_status_via_helper();

int helper::install_service(const std::string& executable_path);
int helper::uninstall_service();
int helper::show_service_status();

int helper::daemon_main();
int helper::worker_main(const std::string& request_path);
```

#### 设计目标

- **只在安装 helper 时需要一次 sudo**
- **日常 `exv` / `exv stop` 以普通用户运行**
- **明文密码仍只在用户侧解密，并通过 root-only 临时请求文件传给 worker**
- **root 生成的用户运行时文件会被 chown 回对应用户**

#### 运行模型

1. `sudo exv service install` 写入 `/Library/LaunchDaemons/com.ecnu.exv.helper.plist`
2. launchd 以 root 身份拉起 `exv __helper-daemon`
3. 普通用户执行 `exv` 时，通过 `/var/run/exv-helper.sock` 发送 JSON 请求
4. daemon 校验发起者 uid/gid，生成 root-only 临时请求文件
5. daemon fork `exv __helper-exec <request-file>`
6. worker 设置 runtime path/owner override 后调用 `vpn::start_with_password()`

`service install` 会把**当前执行中的 `exv` 绝对路径**写入 plist，因此生产环境应从稳定安装路径（如 `/usr/local/bin/exv`）执行该命令。

#### 会话状态

- `/var/run/exv-helper-session.json` 记录当前会话归属用户、配置目录、server、route 数量、retry 配置
- helper 只允许**会话所属用户**查询/停止自己的连接

---

### tunnel — 隧道脚本生成

**文件**：`src/tunnel.hpp` / `src/tunnel.cpp`

```cpp
// 根据 Config.routes 生成 shell 脚本内容
std::string tunnel::generate(const Config& cfg);

// 写入 ~/.ecnuvpn/tunnel.sh 并 chmod +x
bool tunnel::write_script(const Config& cfg);
```

生成的脚本逻辑：
1. 检查 `$reason == "connect"`
2. `ifconfig $TUNDEV $INTERNAL_IP4_ADDRESS ... up`
3. 为 VPN server IP 保留上游默认路由（避免控制连接被自己分流劫持）
4. 循环 `route add <cidr> -interface $TUNDEV`
5. 写入 `route-ready` 标记；若为 helper 模式，额外 `chown` 给用户

---

### logger — 日志系统

**文件**：`src/logger.hpp` / `src/logger.cpp`

```cpp
void logger::init();                 // 确保 log 目录存在
void logger::info(const std::string& msg);
void logger::error(const std::string& msg);
void logger::warn(const std::string& msg);
void logger::show_logs(int lines = 50);  // 读取最近 N 行，着色显示
```

日志格式：`[2026-03-08 12:00:00] [INFO] message`

---

### utils — 工具函数

**文件**：`src/utils.hpp` / `src/utils.cpp`

```cpp
// 彩色输出（ANSI escape codes）
void utils::print_success(const std::string& msg);
void utils::print_error(const std::string& msg);
void utils::print_info(const std::string& msg);
void utils::print_warning(const std::string& msg);
void utils::print_header(const std::string& msg);

// 路径管理
std::string utils::expand_home(const std::string& path);
std::string utils::get_config_dir();   // ~/.ecnuvpn
std::string utils::get_config_path(); // ~/.ecnuvpn/config.json
std::string utils::get_pid_path();    // ~/.ecnuvpn/ecnuvpn.pid
std::string utils::get_log_path();    // ~/.ecnuvpn/ecnuvpn.log
std::string utils::get_tunnel_path(); // ~/.ecnuvpn/tunnel.sh
std::string utils::get_route_ready_path();
std::string utils::get_home_for_uid(uid_t uid);
std::string utils::get_config_dir_for_uid(uid_t uid);
void utils::set_runtime_path_override(const std::string& home,
                                      const std::string& config_dir);
void utils::set_runtime_owner(uid_t uid, gid_t gid);
bool utils::sync_owner(const std::string& path);
std::string utils::get_executable_path();

// 文件 I/O
bool utils::file_exists(const std::string& path);
bool utils::ensure_dir(const std::string& path);   // mkdir -p
std::string utils::read_file(const std::string& path);
bool utils::write_file(const std::string& path, const std::string& content);

// 系统检查
bool utils::check_openconnect();  // which openconnect
bool utils::check_root();         // geteuid() == 0
int utils::run_command(const std::string& cmd);        // system()
std::string utils::run_command_output(const std::string& cmd);  // popen()
```

---

## 数据流

### 密码写入流程

```
exv config set password
    → crypto::read_password_hidden()       [termios echo-off]
    → crypto::encrypt(plaintext, key)      [AES-256-CBC + base64]
    → Config.password = ciphertext
    → config::save(cfg)                    [写入 JSON]
```

### VPN 启动流程

```
exv [no args]
    → config::load()                       [首次运行时生成密钥]
    → vpn::start(cfg)
        → config::get_plaintext_password()
            → crypto::load_key()
            → crypto::decrypt(ciphertext)  [AES-256-CBC 解密]
        → helper::is_available() ?
            helper::start_via_helper(...)
            : vpn::start_with_password(...)
                → tunnel::write_script(cfg)
                → openconnect + optional supervisor
                → tunnel script writes route-ready
```

### helper 启动流程

```
sudo exv service install
    → helper::install_service(executable_path)
    → write /Library/LaunchDaemons/com.ecnu.exv.helper.plist
    → launchctl bootstrap system ...

exv
    → connect /var/run/exv-helper.sock
    → daemon validates peer uid/gid
    → daemon writes root-only request file
    → worker_main(request_file)
        → set_runtime_path_override(user_home, user_config_dir)
        → set_runtime_owner(uid, gid)
        → vpn::start_with_password(...)
```

---

## 安全说明

| 项目 | 说明 |
|------|------|
| 加密算法 | AES-256-CBC with PKCS7 padding |
| 密钥来源 | `CCRandomGenerateBytes` (CommonCrypto) / `/dev/urandom` fallback |
| IV | 每次加密随机生成 16 字节，与密文一起存储 |
| 密钥权限 | `chmod 0600`（仅所有者可读写） |
| 密码在内存 | 解密后以 `std::string` 持有，不做额外 mlock |
| 安全边界 | 防止 config.json 明文泄露；不防御有文件系统访问权限的攻击者 |
