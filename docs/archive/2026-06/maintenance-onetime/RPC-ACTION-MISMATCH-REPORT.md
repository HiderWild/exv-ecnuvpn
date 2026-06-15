# Frontend-Backend RPC Action Name Mismatch Report

## ✅ 已修复

| Frontend (desktop-contract.ts) | Backend (Core RPC) | 状态 | 说明 |
|-------------------------------|-------------------|------|------|
| config.getAuth | config.getAuth | ✅ 已修复 | 刚刚添加映射 |
| config.saveAuth | config.saveAuth | ✅ 已修复 | 刚刚添加映射 |
| config.getSettings | config.getSettings | ✅ 已修复 | 刚刚添加映射 |
| config.saveSettings | config.saveSettings | ✅ 已修复 | 刚刚添加映射 |
| config.getKey | config.getKey | ✅ 已修复 | 刚刚添加映射 |

## ❌ 不匹配（需要修复）

| Frontend (desktop-contract.ts) | Backend (Core RPC) | 问题 |
|-------------------------------|-------------------|------|
| routes.list | route.list | ❌ route vs routes |
| routes.add | route.add | ❌ route vs routes |
| routes.remove | route.remove | ❌ route vs routes |
| routes.reset | ❌ 未注册 | ❌ 缺失 |
| service.status | ❌ 未注册 | ❌ 缺失 |
| helper.status | service.helper_status | ❌ 名称不匹配 |
| runtime.status | ❌ 未注册 | ❌ 缺失 |
| drivers.status | service.driver_status | ❌ 名称不匹配 |
| drivers.install | ❌ 未注册 | ❌ 缺失 |
| logs.list | ❌ 未注册 | ❌ 缺失 |

## ✅ 已匹配（无需修改）

| Frontend | Backend | 状态 |
|----------|---------|------|
| status.get | status.get | ✅ 匹配 |
| vpn.connect | vpn.connect | ✅ 匹配 |
| vpn.disconnect | vpn.disconnect | ✅ 匹配 |

## 🔍 后端独有（前端未使用）

| Backend Action | 说明 |
|----------------|------|
| vpn.status | 可能是legacy API |
| vpn.set_auto_reconnect | 可能是legacy API |
| config.get | legacy名称 |
| config.save | legacy名称 |
| config.get_profile | 未被前端使用 |
| config.save_profile | 未被前端使用 |
| route.disable | 未被前端使用 |
| route.enable | 未被前端使用 |
| service.install | 未被前端使用（通过serviceCommand调用） |
| service.uninstall | 未被前端使用（通过serviceCommand调用） |

## 🎯 修复优先级

### P0 - 导致功能完全无法使用
1. **routes.*** → 路由管理功能完全失效
2. **helper.status** / **service.status** → 服务状态检查失效
3. **logs.list** → 日志查看功能失效

### P1 - 可能影响功能
4. **runtime.status** → 运行时状态检查
5. **drivers.status** / **drivers.install** → 驱动管理

### P2 - 可能未实现
6. **routes.reset** - 需要确认是否实现

## 📋 修复方案

### 方案A：修改Core RPC注册（推荐）
在各个Actions类中添加Desktop API名称的映射

**优点：**
- 前端代码无需改动
- 保持向后兼容（legacy名称仍然有效）

**缺点：**
- Core代码中有重复的注册

### 方案B：修改前端调用
修改desktop-contract.ts中的action名称

**优点：**
- 后端代码简洁

**缺点：**
- 需要修改前端所有调用点
- 可能影响已有的逻辑

## 推荐：方案A

修复所有Core RPC注册，添加Desktop API名称别名。
