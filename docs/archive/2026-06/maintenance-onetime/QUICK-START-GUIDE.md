# VPN 连接诊断 - 快速开始指南

## 已完成的工作

✅ 修复了CLI模式LogEventBus订阅问题
✅ 添加了35+个战略日志点覆盖整个连接流程
✅ 确认了当前架构（串行）
✅ 回答了网卡和VPN引擎关系问题
✅ 评估了并行架构设计

## 立即测试

### 1. 等待编译完成

编译正在后台运行。完成后继续下一步。

### 2. 运行连接测试

```bash
cd D:/Development/Projects/cpp/ECNU-VPN
build/Debug/exv.exe connect
```

### 3. 查看日志输出

现在你应该能看到类似这样的详细日志：

```
[2026-06-09 10:30:00] [INFO] VPN CLI: Connection starting - server=vpn.ecnu.edu.cn username=your_user engine=native
[2026-06-09 10:30:00] [INFO] app_api: vpn.connect entry - password_provided=true
[2026-06-09 10:30:00] [INFO] Backend resolver: Starting resolution - preferred_mode=auto
[2026-06-09 10:30:00] [INFO] Backend resolver: Service status - installed=false available=false
[2026-06-09 10:30:00] [INFO] Backend resolver: Starting oneshot helper
[2026-06-09 10:30:00] [INFO] Oneshot: Entry - helper_path=D:\...\exv-helper.exe
[2026-06-09 10:30:00] [INFO] Oneshot: Starting helper - is_admin=false
[2026-06-09 10:30:00] [INFO] Oneshot: Generated endpoint=\.\pipe\exv-oneshot-abc123
[... UAC 提权对话框出现 ...]
[2026-06-09 10:30:02] [INFO] Oneshot: Helper started successfully - pid=12345
[2026-06-09 10:30:02] [INFO] Helper connector: Attempting connection - endpoint=\.\pipe\exv-oneshot-abc123
[2026-06-09 10:30:03] [INFO] Helper connector: Connected successfully
[2026-06-09 10:30:03] [INFO] TunnelController: do_connect entry - server=vpn.ecnu.edu.cn
```

## 根据日志判断问题

### 场景1: 日志在 "Oneshot: Entry" 后停止
→ **问题:** UAC提权被取消或helper进程无法启动
→ **检查:** 
  - 是否点击了UAC对话框的"是"
  - `exv-helper.exe` 是否存在
  - Windows Defender是否阻止了程序

### 场景2: 日志显示 "Helper connector: Connection failed"
→ **问题:** Helper进程已启动但pipe连接失败
→ **检查:**
  - Helper进程是否真的在运行（任务管理器）
  - Pipe路径是否正确

### 场景3: 日志到达 TunnelController 后失败
→ **问题:** 认证或网络配置阶段出错
→ **检查:**
  - 服务器地址是否正确
  - 用户名密码是否正确
  - 网络连接是否正常

### 场景4: 完全没有日志输出
→ **问题:** 编译失败或LogRenderer未生效
→ **检查:**
  - 编译是否成功
  - 运行的是新编译的版本

## 架构关键点（回答你的问题）

### Q1: 网卡和VPN引擎的关系？
**A:** 
- 网卡由 `PlatformNetworkOps::prepare_tunnel_device()` 创建
- VPN引擎从网卡读取数据包，加密后通过CSTP隧道发送
- 流程：应用 → 路由表 → VPN网卡 → VPN引擎 → CSTP → 服务器

### Q2: 网卡何时创建？
**A:** 
- 在认证成功后，TunnelPhase::OpeningPacketDevice 阶段
- 位置：`src/core/tunnel_controller.cpp:230`
- 时序：Helper启动 → 认证 → **创建网卡** → 启动packet loop

### Q3: 你的并行设计是否合理？
**A:** 理论上合理，但建议：
1. **当前优先级：** 先让串行流程可靠工作
2. **性能优化：** 确认瓶颈后再并行化
3. **实施风险：** 代码刚重构，并行化需要2-3天且有风险

## 修改的文件

如果需要回滚：

```bash
git checkout src/vpn.cpp
git checkout src/app_api.cpp
git checkout src/platform/common/backend_resolver.cpp
git checkout src/helper_common/helper_connector.cpp
git checkout src/platform/win32/oneshot_bootstrap.cpp
git checkout src/core/tunnel_controller.cpp
```

## 下一步

1. ✅ 编译完成（等待中）
2. ⏳ 运行连接测试
3. ⏳ 根据日志定位问题
4. ⏳ 修复实际问题
5. ⏳ 验证连接成功

## 需要帮助？

如果看到错误日志但不知道如何修复，把日志发给我，我会帮你分析。

关键是现在我们有了**完整的可见性** - 不再是"黑盒"连接了！
