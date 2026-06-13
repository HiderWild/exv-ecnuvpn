# 日志和Helper连接问题修复报告

## 问题1：日志不显示 ✅ 已修复

### 问题描述
前端UI中所有日志都不显示，即使C++端有日志输出。

### 根本原因
- C++端通过 `LogEventBus::publish()` 发布日志事件
- `core_process.cpp` 中**没有订阅** `LogEventBus` 并将日志推送到前端
- 前端通过SSE监听 `event.type === 'log'` 接收日志，但从未收到任何日志事件

### 修复内容

**文件：** `src/core/core_process.cpp`

1. **添加头文件：**
```cpp
#include "log_event_bus.hpp"
```

2. **订阅LogEventBus并推送到前端：**
在 `core_process_main()` 中，设置status callback之后添加：

```cpp
// 5b. Subscribe to LogEventBus and push log events to stdout
auto log_subscription = ecnuvpn::LogEventBus::instance().subscribe(
    [](const ecnuvpn::TypedLogEvent& log_event) {
        json log_data = {
            {"level", log_event.level},
            {"message", log_event.message},
            {"timestamp", std::chrono::system_clock::now().time_since_epoch().count()}
        };
        if (!log_event.component.empty()) {
            log_data["component"] = log_event.component;
        }
        if (!log_event.code.empty()) {
            log_data["code"] = log_event.code;
        }
        if (!log_event.fields.empty()) {
            json fields = json::object();
            for (const auto& [key, value] : log_event.fields) {
                fields[key] = value;
            }
            log_data["fields"] = fields;
        }
        json event = {
            {"event", "log"},
            {"data", log_data}
        };
        write_json_line(event);
    }
);
```

### 工作原理
1. C++代码调用 `logger::info()`, `logger::warn()`, `logger::error()`
2. Logger发布 `TypedLogEvent` 到 `LogEventBus`
3. 新增的订阅者捕获事件，转换为JSON格式
4. 通过 `write_json_line()` 发送到stdout
5. Electron的 `CoreRpcClient` 解析JSON，触发 `event === 'log'`
6. 前端 `useSSE.ts` 接收事件，调用 `store.addLog()`
7. 日志显示在UI中

### 状态
✅ 代码已修改  
✅ 编译成功  
✅ 二进制已更新到 `build/windows/electron/bin/exv.exe`

---

## 问题2：Helper Daemon连接失败 🔍 诊断中

### 问题描述
用户报告错误：
```
helper_unavailable: Failed to initialize VPN controller: Failed to connect to helper daemon
```

### 当前状态检查

**Helper进程运行中：** ✅
```
4224624  exv-helper.exe  (PID: 4224624, 启动于 17:12:53)
4234312  exv-helper.exe  (PID: 4234312, 启动于 17:25:24)
4197468  exv.exe         (PID: 4197468, 启动于 17:25:20)
```

有2个helper进程在运行，说明oneshot启动是成功的。

### 架构分析

**连接流程：**
1. `preflight_connect()` 调用 `platform::resolve_backend()`
2. `resolve_backend()` 配置为 `auto` 模式：
   - 先尝试service模式
   - 失败后fallback到oneshot模式
   - 调用 `start_oneshot_helper()` 启动helper（带UAC提权）
3. `start_oneshot_helper()` 返回 `OneshotBackend` 结构（包含endpoint）
4. `initialize_tunnel_controller()` 创建 `HelperConnector`
5. `HelperConnector::connect()` 连接到helper的named pipe

**问题可能在哪里？**

有几种可能：
1. ✅ **Helper启动成功**（进程存在）
2. ❓ **Named pipe端点不匹配**
   - Helper监听: `\\\\.\\pipe\\exv-helper`
   - Connector连接: 可能使用了错误的endpoint
3. ❓ **连接时序问题**
   - Helper刚启动，pipe还未ready
   - Connector立即尝试连接，失败
4. ❓ **权限问题**
   - Helper以管理员权限运行
   - Client以普通权限连接（Windows named pipe权限）

### 需要检查的代码路径

**关键文件：**
- `src/app_api.cpp:380-403` - `initialize_tunnel_controller()` 函数
- `src/helper_common/helper_connector.cpp:17-27` - `PlatformHelperConnector::connect()`
- `src/platform/win32/oneshot_bootstrap.cpp:98+` - `start_oneshot_helper()`

**当前逻辑问题：**
在 `app_api.cpp:382-386`：
```cpp
exv::helper::HelperConnectorConfig cc;
cc.mode = exv::helper::ConnectorMode::Transient;
cc.helper_executable_path = endpoint_override.empty()
                             ? helper_binary_next_to_exv()
                             : endpoint_override;
```

这里设置了 `helper_executable_path` 为**可执行文件路径**，但 `PlatformHelperConnector::resolve_endpoint()` 期望：
- Windows named pipe: `\\\\.\\pipe\\exv-helper`
- 或者可执行文件路径（但它不会启动进程）

**问题根源：**
`PlatformHelperConnector` 在 Transient 模式下**只连接已存在的pipe，不启动helper进程**。

真正的oneshot启动发生在 `resolve_backend()` 中（通过 `start_oneshot_helper()`），但是：
1. `preflight_connect()` 调用 `resolve_backend()` 启动helper
2. 返回backend信息（包含endpoint）
3. 但**没有传递这个endpoint**给 `initialize_tunnel_controller()`
4. `initialize_tunnel_controller()` 使用默认的helper路径尝试连接

### 修复方案

**选项A：传递endpoint（推荐）**
修改 `initialize_tunnel_controller()` 接受endpoint参数：
```cpp
std::shared_ptr<exv::core::TunnelController> 
initialize_tunnel_controller(const std::string& endpoint_override) {
    // ...
    cc.helper_executable_path = endpoint_override; // Should be pipe path, not exe path
    // ...
}
```

并在调用处传递 `backend["endpoint"]` 的值。

**选项B：统一backend resolution**
不要分离 `preflight_connect()` 和 `initialize_tunnel_controller()`：
- 让 `initialize_tunnel_controller()` 内部调用 `resolve_backend()`
- 使用返回的endpoint信息

**选项C：检查时序和重试**
在 `PlatformHelperConnector::connect()` 中：
- 检测Transient模式
- 连接失败时等待并重试（因为helper可能刚启动）
- 添加合理的超时和重试逻辑

### 下一步

需要确认：
1. `preflight_connect()` 返回的 `backend["endpoint"]` 值是什么
2. `initialize_tunnel_controller()` 实际连接到哪个endpoint
3. Helper进程实际监听哪个pipe

然后实施修复方案A或B。

---

## 测试步骤

### 1. 测试日志修复
```powershell
cd webui
pnpm run desktop:dev
```

**预期结果：**
- ✅ 前端UI中显示日志
- ✅ 连接、断开操作有相应日志输出
- ✅ 错误信息显示在日志面板中

### 2. 测试Helper连接
需要等待问题2的修复完成。

---

## 编译状态

✅ **编译成功**  
✅ **二进制更新**：`build/windows/electron/bin/exv.exe`  
⏳ **待测试**：需要启动Electron验证日志显示

## 总结

| 问题 | 状态 | 修复内容 |
|------|------|---------|
| 日志不显示 | ✅ 已修复 | 在core_process.cpp中订阅LogEventBus并推送到前端 |
| Helper连接失败 | 🔍 诊断中 | 识别出endpoint传递问题，需要进一步调查确认 |

**下一步行动：**
1. 启动应用验证日志修复
2. 调查helper endpoint传递逻辑
3. 实施helper连接修复
