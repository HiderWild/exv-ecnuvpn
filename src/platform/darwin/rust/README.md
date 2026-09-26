# macOS Rust 宿主骨架

本目录是独立于根 Common workspace 的 macOS Rust workspace。生产拓扑固定为“Vue UI → Darwin Tauri → 非特权 Darwin Core → 已认证 UDS + gRPC → 按需提升权限的唯一 Darwin Engine → Common WebVPN/CSTP → `utun` 与 Engine 本地数据通路”。Core 只负责 UI-facing 控制、配置与一次性凭据组装、Engine 生命周期和状态投影；它可以请求执行固定 Engine binary，但不执行网络 mutation、不持有 packet FD，也不提供 packet relay。后续 repair 只能在已观察事实下执行受限的 Engine 生命周期修复。Engine 独占网络资源事务、CSTP session、`utun` FD 与数据通路。该路线不依赖 Apple Developer 身份、Network Extension、Swift 或 Xcode。

## 当前行为

- 当前 runtime 仍为 fail-closed，不会伪造连接成功。
- Rust crate 只有一个 Engine runtime owner；尚未接入 CSTP、Engine control 或 packet pump。
- 不链接历史 C++ Darwin 代码；新的 `utun`、路由和 DNS mutation 只能由 Engine 与资源账本实现。
- 不允许引入 `NetworkExtension`、`.appex`、Swift、Xcode build phase 或 signing。

## 本机设置与测试凭据

Tauri 在当前 macOS 用户的应用配置目录保存服务器、学号、路由、MTU 与用户代理；每次启动
普通用户 Core 后会恢复这些设置。密码不写入该设置文件，而是保存到同一用户的 macOS 登录
钥匙串。设置页可以读取、保存或删除学号和密码；删除后不再保留可供本机连接测试使用的凭据。

保留凭据仅表示允许本机的后续连接测试使用它们，不会自动发起网络连接。

## 本地验证

~~~bash
cargo metadata --manifest-path src/platform/darwin/rust/Cargo.toml --format-version 1
cargo check --manifest-path src/platform/darwin/rust/Cargo.toml --workspace --all-targets --locked
cargo test --manifest-path src/platform/darwin/rust/Cargo.toml --workspace --all-targets --locked
~~~

每次构建前的清理及所有工作树共享同一个 `target/` 的规则，以仓库根
第 7 节为准。

