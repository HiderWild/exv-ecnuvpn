# 🎯 Oneshot连接完整修复报告

## 问题回顾

### 原始问题：helper_unavailable错误
**错误信息：**
```
操作失败
helper_unavailable: Failed to initialize VPN controller: Failed to connect to helper daemon
```

### 根本原因分析

问题出在helper进程的启动和连接流程：

1. **Preflight阶段** - 只检查，不启动
   - `preflight_connect`调用`resolve_backend`检查backend可用性
   - 但`start_oneshot=false`，所以不实际启动helper
   - 返回`{"ok": true}`但没有实际启动helper进程

2. **Connect阶段** - 尝试连接不存在的helper
   - `vpn.connect`调用`ensure_tunnel_controller()`
   - `ensure_tunnel_controller`使用`ConnectorMode::Transient`
   - `PlatformHelperConnector`尝试连接到default endpoint
   - **但没有任何helper进程在运行！**
   - 结果：连接失败

### 关键发现

#### HelperConnector的实际行为
查看代码发现：
- `PlatformHelperConnector::connect()` **只负责连接**，不负责启动进程
- `ConnectorMode::Transient`只是一个配置标志，实际上**没有被使用**
- 真正的helper启动由`start_oneshot_helper()`完成（Windows上使用`ShellExecuteEx` + `runas`触发UAC）

#### Backend Resolution的角色
- `resolve_backend()`根据配置决定使用哪种backend（service或oneshot）
- 只有当`start_oneshot=true`时，才会调用`start_oneshot_helper()`
- 启动成功后返回helper的endpoint（如`\\\\.\\pipe\\exv-oneshot-abc123`）

## 完整修复方案

### 修复1：在vpn.connect中启动oneshot helper

**文件：** `src/app_api.cpp` (vpn.connect handler)

**修改：** 在调用`ensure_tunnel_controller`之前，先resolve backend并启动oneshot

```cpp
// ── Backend resolution & oneshot startup ───────────────────────────
platform::BackendResolveOptions backend_opts;
backend_opts.preferred_mode = "auto";
backend_opts.allow_oneshot = true;
backend_opts.start_oneshot = true;  // 实际启动oneshot（触发UAC）
backend_opts.allow_service_start = false;
backend_opts.helper_path = helper_binary_next_to_exv();
nlohmann::json backend = platform::resolve_backend(backend_opts);

// 检查启动是否成功
if (!backend.value("ok", false)) {
    return platform::backend_unavailable_error(backend, ...);
}

// 提取helper endpoint
std::string helper_endpoint = backend.value("endpoint", std::string());

// 使用这个endpoint连接
auto controller = ensure_tunnel_controller(helper_endpoint);
```

**效果：**
1. Service不可用时，`resolve_backend`检测到需要oneshot
2. 调用`start_oneshot_helper()`
3. Windows上使用`ShellExecuteEx`启动`exv-helper.exe --oneshot --pipe \\\\.\\pipe\\exv-oneshot-XXX`
4. 弹出UAC对话框
5. 用户允许后，helper以管理员权限运行
6. 返回helper的endpoint

### 修复2：让ensure_tunnel_controller接受endpoint参数

**文件：** `src/app_api.cpp` (ensure_tunnel_controller函数)

**修改：** 添加endpoint_override参数

```cpp
std::shared_ptr<exv::core::TunnelController> ensure_tunnel_controller(
    const std::string& endpoint_override = "") {
    ...
    exv::helper::HelperConnectorConfig cc;
    cc.mode = exv::helper::ConnectorMode::Transient;
    // 如果提供了endpoint，直接使用（oneshot helper）
    cc.helper_executable_path = endpoint_override.empty()
                                 ? helper_binary_next_to_exv()
                                 : endpoint_override;
    h.client = h.connector->connect(cc);
    ...
}
```

**工作原理：**
- `helper_connector.cpp`的`resolve_endpoint()`函数检查`helper_executable_path`
- 如果它是pipe路径（以`\\\\.\\pipe\\`开头），直接使用它作为endpoint
- 如果是文件路径，使用default endpoint
- 这样oneshot的endpoint就能正确传递给connector

### 修复3：保持preflight简单

**文件：** `src/app_api.cpp` (preflight_connect函数)

**修改：** 保持`start_oneshot=false`，只做检查

```cpp
platform::BackendResolveOptions options;
options.preferred_mode = "auto";
options.allow_oneshot = true;
options.start_oneshot = false;  // 不在preflight启动
options.allow_service_start = false;
options.helper_path = helper_binary_next_to_exv();
nlohmann::json backend = platform::resolve_backend(options);
```

**原因：**
- Preflight只是验证连接前提条件
- 实际的helper启动应该在connect时进行
- 这样UAC提权发生在用户点击"连接"之后，而不是preflight时

## 完整的连接流程

### Service可用时
```
1. vpn.connect
2. resolve_backend(preferred_mode="auto")
3. 检查service状态 → available=true
4. 返回service endpoint
5. ensure_tunnel_controller连接到service
6. 开始VPN连接
```

### Service不可用，Oneshot模式
```
1. 用户点击"连接"按钮
2. vpn.connect handler被调用
3. resolve_backend(start_oneshot=true)
   ├─ 检查service → 不可用
   ├─ start_oneshot=true && allow_oneshot=true
   └─ 调用start_oneshot_helper()
      ├─ 生成随机session_id和auth_token
      ├─ 创建pipe endpoint: \\\\.\\pipe\\exv-oneshot-{session_id}
      ├─ ShellExecuteEx("exv-helper.exe", "--oneshot --pipe ...", runas)
      ├─ 弹出UAC对话框：
      │  "exv-helper.exe想要对您的设备进行更改"
      ├─ 用户点击"是"
      ├─ Helper以管理员权限启动
      ├─ Helper创建named pipe并监听
      └─ 返回{ok:true, endpoint:"\\\\.\pipe\\exv-oneshot-XXX", ...}
4. 提取endpoint
5. ensure_tunnel_controller(endpoint)
   ├─ HelperConnector::connect(helper_executable_path=endpoint)
   ├─ resolve_endpoint检测到这是pipe路径
   ├─ 直接连接到这个endpoint
   └─ 成功！
6. TunnelController使用这个helper连接
7. 开始VPN认证和连接流程
```

## Config保存修复（仍然有效）

**修改：** 移除Core RPC中的Desktop API名称映射

**文件：** `src/core_api/config_actions.cpp`

```cpp
void ConfigActions::register_handlers(AppRpcDispatcher& dispatcher) {
    // 只保留legacy名称
    dispatcher.register_handler("config.get", ...);
    dispatcher.register_handler("config.save", ...);
    
    // 不注册config.getAuth, config.saveAuth等
    // 让它们由app_api.cpp的legacy handlers处理
}
```

## 测试步骤

### 完整测试流程

1. **启动应用**
   ```powershell
   .\start.ps1
   ```

2. **测试密码保存**
   - 进入设置页面
   - 输入服务器、用户名、密码
   - 勾选"记住密码"
   - 点击"保存"
   - **预期：** 保存成功，重新打开设置时密码已保存

3. **测试Oneshot连接**
   - 确保服务未安装或未运行
   - 点击"连接"按钮
   - **预期：** 弹出UAC对话框
   - 点击"是"允许
   - 输入VPN密码（如果密码未保存）
   - **预期：** Helper启动，开始连接流程

## 编译状态
✅ 编译成功  
✅ 二进制已更新

## 预期测试结果

- ✅ 密码保存功能正常
- ✅ 点击连接弹出UAC对话框
- ✅ 允许UAC后helper启动成功
- ✅ VPN连接流程开始
- ✅ 不再报错"helper_unavailable"

---
**最后更新：** 2026-06-09  
**状态：** 等待用户测试
