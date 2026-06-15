# 🎉 密码保存和Oneshot连接修复报告

## 问题诊断

### 问题1：密码保存失效 ❌
**现象：** 设置页面输入密码并保存后，文本框仍提示"请输入密码"

**根本原因：**  
Core RPC的`config_actions.cpp`注册了stub handlers覆盖了app_api.cpp的真实实现：
- `config.saveSettings` → stub实现（只返回success，不做任何操作）
- 真正的实现在`app_api.cpp`的legacy handlers中

**修复方案：**  
移除Core RPC中的Desktop API名称注册，让这些action继续使用app_api.cpp的legacy handlers

### 问题2：Oneshot连接失败 ❌
**现象：** 点击连接后输入密码，报错：
```
操作失败
oneshot_not_supported: One-shot helper is available only when explicitly requested.
```

**根本原因：**  
在`preflight_connect`中只设置了`allow_oneshot=true`，但没有设置`start_oneshot=true`。

根据`backend_resolver.cpp`的逻辑：
```cpp
// 只有同时满足两个条件才会启动oneshot
if (options.start_oneshot && options.allow_oneshot) {
    // 启动oneshot helper (带UAC提权)
    backend = deps.start_oneshot_helper(...);
}
// 否则返回错误
if (options.allow_oneshot || options.preferred_mode == "oneshot") {
    return unavailable(kOneshotNotSupportedCode, ...);
}
```

**修复方案：**  
在`preflight_connect`中设置`start_oneshot=true`，实际启动oneshot helper并触发UAC提权

## 已应用的修复

### 1. src/core_api/config_actions.cpp
**修改：** 移除Desktop API名称的映射
```cpp
void ConfigActions::register_handlers(AppRpcDispatcher& dispatcher) {
    // 只保留legacy名称
    dispatcher.register_handler("config.get", ...);
    dispatcher.register_handler("config.save", ...);
    
    // 移除了：config.getAuth, config.saveAuth, 
    //         config.getSettings, config.saveSettings, config.getKey
    // 这些由app_api.cpp的legacy handlers处理
}
```

**效果：**  
- `config.saveSettings` 现在使用app_api.cpp中的真实实现
- 能够正确保存密码到配置文件（加密存储）

### 2. src/app_api.cpp (preflight_connect)
**修改：** 启用oneshot helper启动
```cpp
platform::BackendResolveOptions options;
options.preferred_mode = "auto";
options.allow_oneshot = true;
options.start_oneshot = true;     // 新增：实际启动oneshot
options.allow_service_start = false;
options.helper_path = helper_binary_next_to_exv();
```

**效果：**  
- preflight阶段就启动oneshot helper进程（带UAC提权）
- 弹出UAC提权对话框
- 用户允许后，helper以管理员权限运行
- vpn.connect可以使用这个已经运行的helper进程

## Routes和Service的RPC映射（保持不变）

这些修复仍然有效：
- ✅ Routes: routes.list/add/remove/reset
- ✅ Service: service.status, helper.status
- ✅ Drivers: drivers.status, drivers.install

## 测试步骤

### 1. 测试密码保存
1. 启动应用：`.\start.ps1`
2. 进入设置页面
3. 输入服务器、用户名、密码
4. 勾选"记住密码"
5. 点击"保存设置"
6. **预期：** 设置页面关闭，密码保存成功
7. 重新打开设置页面
8. **预期：** 密码框显示已保存（不显示"请输入密码"）

### 2. 测试Oneshot连接
1. 确保服务未安装（或未运行）
2. 点击"连接"按钮
3. **预期：** 弹出UAC对话框："exv-helper.exe想要对您的设备进行更改"
4. 点击"是"允许
5. 输入VPN密码（如果之前没保存）
6. **预期：** 
   - Helper以管理员权限启动
   - 开始VPN连接流程
   - 不再报错"oneshot_not_supported"

## 技术细节

### Backend Resolution流程

**Service可用时：**
```
preferred_mode="auto" 
→ 检查service状态 
→ service.available=true 
→ 返回service backend
```

**Service不可用，Oneshot模式：**
```
preferred_mode="auto"
→ service不可用
→ start_oneshot=true && allow_oneshot=true
→ 调用start_oneshot_helper()
  → 在Windows上启动exv-helper.exe (带ShellExecute + runas)
  → 弹出UAC对话框
  → 用户允许后以管理员权限运行
→ 返回oneshot backend (包含pipe endpoint)
```

### Config保存流程

**修复前：**
```
前端: config.saveSettings
→ Desktop API: window.ecnuVpn.config.saveSettings()
→ Core RPC: config.saveSettings
→ config_actions.cpp stub (只返回success，不保存)
→ 配置丢失 ❌
```

**修复后：**
```
前端: config.saveSettings
→ Desktop API: window.ecnuVpn.config.saveSettings()
→ Core RPC: 未注册，fallback到legacy handler
→ app_api.cpp legacy handler
  → ConfigManager.save()
  → 加密密码
  → 写入配置文件
→ 配置持久化 ✅
```

## 编译状态
✅ 编译成功  
✅ 二进制已更新

## 后续测试

请运行`.\start.ps1`并测试：
1. 密码保存功能
2. Oneshot连接（UAC提权）
3. 服务安装弹窗响应（之前已修复）

---
**最后更新：** 2026-06-09  
**状态：** 等待用户测试
