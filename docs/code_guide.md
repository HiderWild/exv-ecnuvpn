# EXV 代码文档

> 面向开发者的项目结构与 API 参考。

---

## 项目结构

```
ECNU-VPN/
├── CMakeLists.txt              # C++17 构建，注入版本号 + 嵌入 WebUI 资产
├── .gitignore
├── scripts/
│   └── embed_assets.py         # 将 WebUI dist 编译为 C++ header
├── docs/
│   ├── user_guide.md           # 用户使用手册
│   └── code_guide.md           # 本文档
├── include/
│   ├── httplib.h               # cpp-httplib (HTTP server)
│   └── nlohmann/json.hpp       # Header-only JSON 库 v3.11.3
├── webui/                      # Vue 3 + TypeScript + Vite 前端
│   ├── src/                    # SPA 源码
│   └── dist/                   # 构建产物（嵌入到 C++ binary）
└── src/
    ├── main.cpp                # CLI 入口 + WebUI 启动
    ├── config.hpp/cpp          # 配置管理（含 WebUI 字段）
    ├── config_api.hpp/cpp      # REST API 处理
    ├── config_manager.hpp/cpp  # 运行时配置读写
    ├── crypto.hpp/cpp          # AES-256-CBC 加密 + 密钥管理
    ├── helper.hpp/cpp          # launchd root helper / IPC / service 管理
    ├── vpn.hpp/cpp             # VPN 控制（start/stop/status）
    ├── tunnel.hpp/cpp          # tunnel.sh 动态生成
    ├── logger.hpp/cpp          # 日志系统
    ├── sse_broadcaster.hpp/cpp # SSE 实时推送（日志 + 状态）
    ├── webui.hpp/cpp           # HTTP 服务器 + REST + SSE 路由
    └── utils.hpp/cpp           # 工具函数
```

---

## 构建系统

### CMakeLists.txt

- C++17 标准 + pthread（cpp-httplib 需要）
- 通过 `target_compile_definitions` 注入 `ECNUVPN_VERSION` 和 `EMBEDDED_ASSETS`
- 源文件：`src/*.cpp`（12 个文件）
- 依赖：`include/nlohmann/json.hpp`、`include/httplib.h`（header-only）
- 系统依赖：`CommonCrypto`（macOS 内置）
- 自定义 target `embed_assets`：运行 `scripts/embed_assets.py` 将 `webui/dist/` 编译为 `src/webui_assets.hpp`

```bash
cmake -B build -DCMAKE_BUILD_TYPE=Release
cmake --build build -j$(sysctl -n hw.ncpu)
sudo cmake --install build        # 安装到 /usr/local/bin
sudo exv service install          # 安装 launchd root helper
```

---

## 模块 API

### crypto — 加密与密钥管理

**文件**：`src/crypto.hpp` / `src/crypto.cpp`

```cpp
std::string crypto::generate_key();           // 32 字节随机密钥 → 64 位 hex
std::string crypto::load_key();               // 从 ~/.ecnuvpn/.key 读取
bool crypto::save_key(const std::string& hex_key);
bool crypto::validate_key(const std::string& hex_key);
std::string crypto::init_key_if_needed();     // 不存在则生成
bool crypto::reset_key();                     // 交互确认 + 清除密码密文
std::string crypto::key_status();             // "valid" | "missing" | "corrupt"
std::string crypto::key_path();

std::string crypto::encrypt(const std::string& plaintext, const std::string& hex_key);
std::string crypto::decrypt(const std::string& ciphertext_b64, const std::string& hex_key);
std::string crypto::read_password_hidden(const std::string& prompt);
```

算法：AES-256-CBC (CommonCrypto)，密文格式 `base64(IV[16] + ciphertext)`。

---

### config — 配置管理

**文件**：`src/config.hpp` / `src/config.cpp`

```cpp
struct Config {
    std::string server;
    std::string username;
    std::string password;    // AES-256-CBC 密文
    int mtu = 1290;
    std::string useragent;
    bool disable_dtls = true;
    bool remember_password = true;
    std::vector<std::string> routes;
    std::vector<std::string> extra_args;
    std::string log_file;
    int webui_port = 18080;
    std::string webui_bind = "127.0.0.1";
    bool webui_enabled = true;
};

Config config::load();
bool config::save(const Config& cfg);
void config::show(const Config& cfg);
std::string config::get_plaintext_password(const Config& cfg);
Config config::import_from(const std::string& path);
bool config::set_value(Config& cfg, const std::string& key);
Config config::reset();
bool config::add_route(Config& cfg, const std::string& route);
bool config::remove_route(Config& cfg, const std::string& route);
void config::list_routes(const Config& cfg);
void config::key_show();
bool config::key_reset();
```

---

### vpn — VPN 控制

**文件**：`src/vpn.hpp` / `src/vpn.cpp`

```cpp
int vpn::start(const Config& cfg, int retry_limit = 0);
int vpn::start_with_password(const Config& cfg, const std::string& plaintext_password, int retry_limit = 0);
int vpn::stop();
int vpn::status();
```

`start()` 流程：
1. openconnect 检查（未安装则提示 Homebrew 安装）
2. 配置验证
3. 密码解密
4. 优先走 helper（`helper::is_available()`）
5. root 直连或 helper worker → `start_with_password()`
6. 生成 tunnel.sh → fork openconnect → 可选 supervisor → 轮询 route-ready

---

### helper — launchd root helper 与 IPC

**文件**：`src/helper.hpp` / `src/helper.cpp`

```cpp
bool helper::is_available();
bool helper::start_via_helper(const Config& cfg, const std::string& plaintext_password, int retry_limit);
bool helper::stop_via_helper();
bool helper::show_status_via_helper();
int helper::install_service(const std::string& executable_path);
int helper::uninstall_service();
int helper::show_service_status();
int helper::daemon_main();
int helper::worker_main(const std::string& request_path);
```

设计要点：
- 安装 helper 时仅需一次 sudo
- 日常 `exv` / `exv stop` 以普通用户运行
- Socket `/var/run/exv-helper.sock` 权限 root:staff 0660，所有 macOS 用户可达
- 明文密码仅在用户侧解密，通过 root-only 临时文件传给 worker
- `send_request()` 带 select() 超时（默认 15s，start 用 120s）

运行模型：
1. `sudo exv service install` → 写 plist + `launchctl bootstrap`
2. launchd 以 root 拉起 `exv __helper-daemon`
3. 用户 `exv` → connect socket → JSON 请求
4. daemon fork `exv __helper-exec <request-file>` → worker 调用 `vpn::start_with_password()`

---

### webui — HTTP 服务器与 REST API

**文件**：`src/webui.hpp` / `src/webui.cpp`

```cpp
class WebUIServer {
    WebUIServer(ConfigManager& config_mgr, SseBroadcaster& log_bc, SseBroadcaster& status_bc, int port, const std::string& bind);
    void start();
    void stop();
};
```

路由：
- `/` — 嵌入式前端 HTML/JS
- `/api/config` — GET 配置 / PUT 更新配置
- `/api/start` — POST 启动 VPN
- `/api/stop` — POST 停止 VPN
- `/api/status` — GET VPN 状态
- `/sse/logs` — SSE 实时日志流
- `/sse/status` — SSE 实时状态流

---

### sse_broadcaster — SSE 推送

**文件**：`src/sse_broadcaster.hpp` / `src/sse_broadcaster.cpp`

```cpp
class SseBroadcaster {
    SseBroadcaster(const std::string& source_path, std::function<std::string()> poll_fn, int max_clients);
    void start();
    void stop();
    void broadcast(const std::string& event_type, const std::string& data);
};
```

日志 SSE：tail 日志文件 + 定期轮询新内容。
状态 SSE：定期通过 helper socket 查询 VPN 状态。

---

### config_manager / config_api — 运行时配置

**文件**：`src/config_manager.hpp/cpp`、`src/config_api.hpp/cpp`

`ConfigManager` 封装配置目录的读写操作，`ConfigAPI` 提供 REST endpoint 的处理逻辑。

---

### tunnel — 隧道脚本生成

**文件**：`src/tunnel.hpp` / `src/tunnel.cpp`

```cpp
std::string tunnel::generate(const Config& cfg);
bool tunnel::write_script(const Config& cfg);
```

脚本逻辑：检查 `$reason == "connect"` → ifconfig → 保留上游路由 → route add 循环 → 写 route-ready → chown 给用户（helper 模式）

---

### logger — 日志系统

**文件**：`src/logger.hpp` / `src/logger.cpp`

```cpp
void logger::init();
void logger::info/error/warn(const std::string& msg);
void logger::show_logs(int lines = 50);
```

格式：`[2026-03-08 12:00:00] [INFO] message`

---

### utils — 工具函数

**文件**：`src/utils.hpp` / `src/utils.cpp`

彩色输出、路径管理、文件 I/O、系统检查、流量统计（`get_interface_traffic`）等。

---

## 数据流

### VPN 启动流程

```
exv
    → config::load()
    → vpn::start(cfg)
        → helper::is_available() ?
            helper::start_via_helper(cfg, password, retry)
                → send_request({"action":"start"}, timeout=120s)
                → daemon fork worker → vpn::start_with_password()
            : vpn::start_with_password(cfg, password, retry)
                → tunnel::write_script(cfg)
                → openconnect + optional supervisor
    → WebUI fork (background) or foreground
        → WebUIServer.start()
        → SseBroadcaster.start() (log + status)
```

### helper 通信

```
exv → connect /var/run/exv-helper.sock (root:staff 0660)
    → send JSON + '\n' + shutdown(SHUT_WR)
    → select(timeout) → read response
    → daemon: getpeereid() → validate → fork handler → process_client_request()
```

---

## 安全说明

| 项目 | 说明 |
|------|------|
| 加密算法 | AES-256-CBC with PKCS7 padding |
| 密钥来源 | `CCRandomGenerateBytes` (CommonCrypto) |
| IV | 每次加密随机 16 字节 |
| 密钥权限 | `chmod 0600` |
| Helper socket | `root:staff 0660` — 所有本地用户可达 |
| 密码传输 | 用户侧解密 → root-only 临时文件 → worker |
