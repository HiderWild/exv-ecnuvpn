# ✅ 最终修复验证清单

## 已完成的修复

### 1. 日志系统 ✅
**文件:** `src/core/core_process.cpp`
- 添加 `LogEventBus` 订阅
- 将日志转换为JSON并推送到stdout
- 格式: `{"event":"log","data":{...}}`

### 2. Named Pipe路径 ✅
**文件:** `src/platform/win32/oneshot_bootstrap.cpp:111`

**修复前:**
```cpp
backend.endpoint = "\\\\.\\pipe\\exv-oneshot-" + session_id;
// 结果: \\.\pipe<ESC>xv-oneshot-...（包含ESC字符）
```

**修复后:**
```cpp
backend.endpoint = "\\\\.\\pipe\\\\exv-oneshot-" + session_id;
// 结果: \\.\pipe\exv-oneshot-...（正确）
```

## 编译和部署状态

- ✅ 编译完成: Jun 9 18:08
- ✅ 二进制部署: `build/windows/electron/bin/`
- ✅ 旧进程清理完成

## 验证步骤

### 期望看到的内容:

1. **控制台日志** (stdout/stderr)
   ```
   [DEBUG] Oneshot endpoint: \\.\pipe\exv-oneshot-abc12345
   [DEBUG] Helper args: --oneshot --pipe "\\.\pipe\exv-oneshot-abc12345" ...
   ```

2. **JSON日志事件**
   ```json
   {"event":"log","data":{"level":"INFO","message":"Starting oneshot helper..."}}
   {"event":"log","data":{"level":"INFO","message":"Helper connected successfully"}}
   ```

3. **连接过程**
   - UAC提权提示出现
   - Helper进程启动
   - 连接成功
   - 状态变为"已连接"

### 如果仍然失败:

查看控制台输出中的:
- `[DEBUG] Oneshot endpoint:` 后的路径是否包含不可见字符
- 是否有 "Failed to connect" 错误
- Helper进程是否启动（检查任务管理器）

## 测试命令

```powershell
# 方式1: 使用测试脚本
.\test-with-debug-output.ps1

# 方式2: 直接启动
cd webui
pnpm run desktop:dev
```

## 如果测试成功

连接成功后可以：
1. 查看UI日志面板中的日志
2. 验证VPN连接功能
3. 测试断开连接
4. 清理调试日志（可选）

## 调试输出位置

- **C++调试日志**: stderr（控制台红色输出）
- **JSON事件**: stdout（控制台正常输出）
- **Electron日志**: 控制台DevTools

---

**状态:** ⏳ 等待用户测试反馈  
**最后更新:** 2026-06-09 18:08
