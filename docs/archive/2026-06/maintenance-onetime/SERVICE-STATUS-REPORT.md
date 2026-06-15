# 服务安装问题分析报告

## 问题状态

### ✅ 已修复：UI响应延迟
- UI现在响应迅速，没有30秒延迟
- Core进程stdin循环正常工作

### ⚠️ 服务安装未实现
- **问题：** 点击"安装服务"报错
- **根本原因：** `service.install` 和 `service.uninstall` 是stub实现

## 技术分析

### 当前代码状态

**文件：** `src/core_api/service_actions.cpp:31-47`

```cpp
RpcResponse ServiceActions::install_helper(const RpcRequest& req) {
    RpcResponse resp;
    // Stub
    resp.success = false;
    resp.error_code = "not_implemented";
    resp.error_message = "Helper installation not yet implemented";
    return resp;
}

RpcResponse ServiceActions::uninstall_helper(const RpcRequest& req) {
    RpcResponse resp;
    // Stub
    resp.success = false;
    resp.error_code = "not_implemented";
    resp.error_message = "Helper uninstallation not yet implemented";
    return resp;
}
```

### ✅ 替代方案：Oneshot模式（单次连接）

Windows平台**已实现**oneshot模式，无需安装服务即可使用：

**工作原理：**
1. 每次连接时临时启动 `exv-helper.exe`（提权）
2. 使用named pipe通信
3. 连接结束后helper进程退出

**实现位置：** `src/platform/win32/oneshot_bootstrap.cpp:98-155`

**优点：**
- ✅ 无需安装Windows服务
- ✅ 无需处理服务权限和生命周期管理
- ✅ 每次使用时弹出UAC提权提示（更安全）

**缺点：**
- ⚠️ 每次连接需要UAC提权
- ⚠️ 略慢于持久服务（需要启动helper进程）

### 必需文件

Oneshot模式需要两个二进制文件在同一目录：
- ✅ `exv.exe` - Core进程（已编译）
- ✅ `exv-helper.exe` - Helper进程（已编译）

**当前位置：** `build/windows/electron/bin/`

## 用户使用建议

### 单次连接（推荐，已可用）

1. 启动应用
2. 点击"连接"按钮
3. 允许UAC提权提示
4. 应用会自动使用oneshot模式连接

**预期行为：**
- 第一次连接时弹出UAC
- 连接成功
- 断开后helper进程自动退出

### 服务模式（未实现）

如果需要实现持久服务模式，需要：

1. **实现 `service.install`** 在 `src/core_api/service_actions.cpp`
   - Windows: 使用 `sc.exe create` 或 Win32 API
   - 注册服务指向 `exv-helper.exe --service`
   - 设置自动启动

2. **实现 `service.uninstall`**
   - 停止服务
   - 使用 `sc.exe delete` 删除服务

3. **服务生命周期管理**
   - 服务启动/停止
   - 错误恢复
   - 日志记录

## 测试步骤

### 测试Oneshot模式

```powershell
cd webui
pnpm run desktop:dev
```

**操作：**
1. 点击"连接"按钮（不要点"安装服务"）
2. 允许UAC提权
3. 检查连接状态

**预期结果：**
- ✅ UAC提示出现
- ✅ 连接成功
- ✅ 控制台无错误

**如果失败：**
- 检查 `build/windows/electron/bin/exv-helper.exe` 是否存在
- 查看控制台日志中的错误码：
  - `oneshot_not_supported`: helper路径未正确传递
  - `service_start_failed`: helper启动失败
  - `oneshot_elevation_denied`: 用户拒绝UAC

### 预期的服务安装错误（正常）

**操作：** 点击"安装服务"
**结果：** ❌ 报错 "not_implemented"
**原因：** 功能尚未实现
**影响：** 无，使用oneshot模式即可

## 开发建议

如果需要实现服务模式，参考：
- `src/platform/win32/helper_service_manager.cpp:88-95` - 已有服务路径构建逻辑
- Windows服务API文档
- 或者保持现状，仅使用oneshot模式（更简单，更安全）

## 总结

| 功能 | 状态 | 说明 |
|------|------|------|
| UI响应 | ✅ 已修复 | 无延迟，立即响应 |
| 单次连接（oneshot） | ✅ 可用 | 推荐使用，每次弹UAC |
| 服务安装 | ❌ 未实现 | Stub代码，返回not_implemented |
| 服务运行 | ❌ 未实现 | 依赖服务安装 |

**当前推荐使用方式：** Oneshot单次连接模式（无需安装服务）
