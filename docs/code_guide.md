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
- 源文件：`src/*.cpp`（7 个文件）
- 依赖：`include/nlohmann/json.hpp`（header-only）
- 系统依赖：`CommonCrypto`（macOS 内置，无需链接额外 framework）

```bash
mkdir build && cd build
cmake .. -DCMAKE_BUILD_TYPE=Release
make -j$(sysctl -n hw.ncpu)
sudo cmake --install .          # 安装到 /usr/local/bin
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
int vpn::start(const Config& cfg);   // 连接 VPN
int vpn::stop();                     // 断开 VPN
int vpn::status();                   // 显示状态
```

#### `start()` 流程

1. **openconnect 检查**：未安装则提示安装（Homebrew 流程）
2. **sudo 检查**：非 root 则退出
3. **重复连接检查**：`pgrep -x openconnect`
4. **配置验证**：username / password 非空
5. **密码解密**：`config::get_plaintext_password(cfg)`
6. **生成 tunnel.sh**：`tunnel::write_script(cfg)`
7. **构建命令**：`echo '<password>' | openconnect ... -b --pid-file=...`
8. **执行 + 验证**：等待 500ms 后检查 PID

#### PID 管理

- `~/.ecnuvpn/ecnuvpn.pid` 存储 openconnect PID
- `stop()` 优先读 PID 文件，失败时 fallback 到 `pgrep`
- 先发 SIGTERM（等 1.5s），再发 SIGKILL（等 1s）

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
3. 循环 `route add <cidr> -interface $TUNDEV`

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
ecnuvpn config set password
    → crypto::read_password_hidden()       [termios echo-off]
    → crypto::encrypt(plaintext, key)      [AES-256-CBC + base64]
    → Config.password = ciphertext
    → config::save(cfg)                    [写入 JSON]
```

### VPN 启动流程

```
ecnuvpn [no args]
    → config::load()                       [首次运行时生成密钥]
    → vpn::start(cfg)
        → config::get_plaintext_password()
            → crypto::load_key()
            → crypto::decrypt(ciphertext)  [AES-256-CBC 解密]
        → tunnel::write_script(cfg)        [动态生成 tunnel.sh]
        → system("echo '<pw>' | openconnect ...")
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
