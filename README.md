# EXV - 华东师范大学智能 VPN 客户端

EXV 是面向华东师范大学校园网的跨平台 VPN 客户端。它的目标是连接校园 VPN 后，只让校内资源流量进入 VPN 隧道，其余互联网流量继续走本地网络。

## 公开镜像说明

这个仓库是过滤后的公开源码镜像，只保留源码、公开契约、资产、发行默认配置和这个中文 README。开发文档、测试、脚本、自动化工作区、agent 工作产物和一次性维护材料不包含在公开仓库中。

## 主要能力

- 分流路由：仅校园网段走 VPN，普通互联网访问保持本地出口。
- 原生 VPN core：C++ core 负责认证、协议状态、连接生命周期、重连策略和数据面协调。
- native WebView 桌面壳：Windows 使用 WebView2，macOS 使用 WKWebView，Linux 使用 WebKitGTK。
- 特权 helper：以系统服务或一次性提权进程执行网卡、DNS、路由和清理等特权操作。
- 稳定 RPC 契约：桌面 UI 通过 core RPC 和 helper contract 获取状态、触发连接、管理配置。

## 代码结构

- `src/`：C++ core、helper、平台适配、native WebView shell 和 VPN 协议实现。
- `include/`：项目使用的头文件依赖。
- `webui/src/`：Vue 3 + TypeScript renderer 源码。
- `webui/desktop/`、`webui/host/`：桌面桥接契约源码。
- `contracts/`：core、helper、UI 之间的公开契约元数据。
- `assets/`：应用图标和界面资产。
- `distribution/`：ECNU 发行默认配置。

## 技术栈

- C++20
- Vue 3 + TypeScript
- native WebView shell：WebView2 / WKWebView / WebKitGTK
- nlohmann/json
- cpp-httplib
- Wintun / utun / Linux network stack
