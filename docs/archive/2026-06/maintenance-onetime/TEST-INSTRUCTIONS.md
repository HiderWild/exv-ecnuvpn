# 测试指令

## 已完成的修改

### 1. Electron端（已编译）
- **移除了 `createInterface`** - 不再使用readline
- **直接处理raw stdout/stderr** - 手动缓冲和行分割
- **添加了详细的诊断日志** - 每一步都有[CoreRpc]日志

### 2. 理论依据
根据systematic-debugging和调试历史分析：
- ✅ 独立Node.js测试脚本**工作正常**（stdin/stdout机制本身没问题）
- ❌ Electron环境**超时30秒**
- 📊 调试文档指出：**Node.js的readline或Stream缓冲问题** (60%概率)

**核心问题：** `createInterface`可能在Electron环境中有缓冲延迟

**修复：** 完全绕过readline，直接监听stdout.on('data')并手动处理换行符

## 测试步骤

### 方法1：直接启动Electron（推荐）

```powershell
# 1. 停止所有现有进程
Get-Process -Name "exv","electron" -ErrorAction SilentlyContinue | Stop-Process -Force

# 2. 启动Electron开发模式
cd webui
pnpm run desktop:dev
```

### 方法2：使用测试脚本

```powershell
.\test-electron-communication.ps1
```

## 预期结果

### 如果修复成功，你应该看到：
```
[CoreRpc] Constructor called, PID: XXXX
[CoreRpc] setupReadline() called
[CoreRpc] stdout available: true
[CoreRpc] All event listeners attached (bypassing readline)
[CoreRpc] request() called: { action: 'status.get', ... }
[CoreRpc] Writing to stdin: ...
[CoreRpc] Write SUCCESS for id: 1
[CoreRpc] STDOUT RAW chunk received: XXX bytes
[CoreRpc] STDOUT RAW content: {"id":1,"ok":true,...}
[CoreRpc] Extracted line: {"id":1,"ok":true,...}
[CoreRpc] handleLine() called, trimmed length: XXX
[CoreRpc] JSON parsed successfully: ...
[CoreRpc] Handling response for id: 1
[CoreRpc] Response is success, resolving
```

### 如果仍然失败：
```
[CoreRpc] TIMEOUT: request status.get (id=1) after 30000ms
```

## 下一步

**如果成功：**
- 问题确认是readline缓冲导致的
- 可以移除所有调试日志，保留直接处理raw data的代码
- 验证所有功能（连接、断开、状态更新等）

**如果仍然超时：**
- 检查是否有[CoreRpc] STDOUT RAW日志（证明C++端有输出）
- 检查是否有[CoreRpc] Write SUCCESS日志（证明写入成功）
- 如果有STDOUT RAW但没有Extracted line，说明换行符处理有问题
- 如果连STDOUT RAW都没有，问题在C++端（但独立测试工作，很奇怪）

## 独立测试脚本（已验证工作）

如果需要对比，运行：
```powershell
node test-stdin-communication.js
```

这个脚本已验证**通信机制本身是正常的**。
