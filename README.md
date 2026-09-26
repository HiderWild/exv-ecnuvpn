# EXV

EXV 是华东师范大学校园 VPN（ECNU VPN / aTrust 之外亦支持 Cisco 型 WebVPN）的
开源客户端，提供 Windows 与 macOS 双平台桌面应用。

- **Windows**：Tauri 桌面应用 + 后台引擎服务，支持 TUN / 系统代理两种连接模式，
  内置校园分流路由与网关旁路。
- **macOS**：同构的桌面应用与 utun 隧道（darwin 侧发布物随后补齐）。

本仓库为 4.x Rust 产品线。相关能力与已知限制见 `CHANGELOG.md`。

## 目录结构

```text
src/common/rust/            平台无关核心（协议、配置、运行时）
src/platform/win32/rust/    Windows 引擎/宿主/UI（Tauri）
src/platform/win32/windows_setup_rust/   安装器与载荷打包工具
src/platform/darwin/rust/   macOS 引擎/宿主/UI
proto/                      进程间与控制协议定义
runtime/win32-x64/          第三方运行时组件（Wintun）
scripts/                    构建与打包脚本
```

## 从源码构建（Windows）

前置环境（本仓库不代管工具链安装）：

- Rust（rustup，含 cargo）
- Node.js ≥ 20 与 npm
- CMake + Ninja（构建安装器与打包工具）
- PowerShell（Windows 自带）或 pwsh
- Python 3（载荷自检用）

一键构建安装包（在仓库根执行）：

```powershell
powershell -ExecutionPolicy Bypass -File scripts\build-release.ps1
```

脚本会在干净的临时 worktree 中完成 前端构建 → 三件套组装 → 安装器打包 →
载荷自检，成功后输出安装包路径、SHA256 与检验报告（默认位于 `build\release\`）。
脚本只构建已提交的内容，未提交的改动不会进入安装包。

开发调试（不打包）：

```powershell
cd src\platform\win32\rust\tauri\frontend
npm ci
cd ..
cargo cargo check --workspace   # 核心与平台工作区按各自 Cargo.toml 分别检查
```

## 许可

本项目以 MIT 许可发布（见 `LICENSE`）。`runtime/win32-x64/wintun.dll` 为
Wintun 官方预编译签名组件，其分发条款见同目录 `README.txt`。
