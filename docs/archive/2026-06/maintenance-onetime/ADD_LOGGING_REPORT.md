# VPN Connection Logging Enhancement

## 修改计划

### 文件1: src/vpn.cpp
**目的：** 启用CLI模式的LogEventBus订阅，添加连接入口日志

**修改：**
1. 添加 #include "log_renderer.hpp"
2. 在 start() 函数开头创建 LogRenderer 实例
3. 添加连接开始、配置验证、结果日志

### 文件2: src/app_api.cpp
**目的：** 添加 vpn.connect 处理器的关键阶段日志

**修改位置：**
- Line 609: vpn.connect 入口
- Line 623: preflight 调用前后
- Line 713: ensure_tunnel_controller 前后
- Line 731: controller->connect 调用

### 文件3: src/platform/common/backend_resolver.cpp
**目的：** 记录后端解析决策过程

**修改位置：**
- Line 44: 服务状态检查
- Line 65: oneshot helper 启动前

### 文件4: src/platform/win32/oneshot_bootstrap.cpp
**目的：** 替换 stderr 调试输出为结构化日志

**修改位置：**
- Line 99: 函数入口
- Line 117-118: 替换 std::cerr
- Line 121/127: 启动模式
- Line 151: 等待 hello

### 文件5: src/helper_common/helper_connector.cpp
**目的：** 记录 Helper 连接尝试

**修改位置：**
- Line 17: connect() 入口
- Line 24: 连接结果

### 文件6: src/core/tunnel_controller.cpp
**目的：** 记录连接状态机入口

**修改位置：**
- Line 137: do_connect() 入口

