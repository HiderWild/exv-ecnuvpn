# Darwin 特权服务代理（exv-vpn-darwin-service-agent）

## 定位与边界（2026-09-20 起）

本组件是**产品线自有的特权服务代理**：darwin 侧唯一被允许写 `/Library/LaunchDaemons`、
维护 `/Library/Application Support/EXV/ServiceAgent` 并调用 `launchctl` 的产品组件。
它由 core 经 `osascript … with administrator privileges` 以固定动词调用，承担四件事：

1. 安装/替换系统服务（代理 daemon job + service engine job）；
2. 无服务连接时的一次性 root 拉起（`start-engine-once`）；
3. 启动/修复服务（`start`）；
4. 卸载与旧残留回收（`uninstall` / `retire-legacy` / `sweep-runtime-residue`）。

**“开发伴侣”已停用并退役**：原 `tools/darwin-dev-companion`（二进制
`exv-darwin-dev-companion`）是开发期工具，却被生产路径当作事实上的特权服务代理使用。
用户硬性指令是「生产严禁依赖/调用/随附开发工具」；2026-09-20 完成的解耦把它整体迁入
产品区、按生产语义重命名，并让本代理在 install / uninstall / 预卸载三条入口上
**幂等回收旧 `DevCompanion` 命名空间**（label `com.exv.vpn.dev-companion`、其 plist 与
`/Library/Application Support/EXV/DevCompanion` 目录）。不再存在第二份实现，因此不会
出现“生产与开发两个副本漂移”的问题。

开发与测试**直接使用本代理**（形态与生产完全一致）：

```bash
cargo build -p exv-vpn-darwin-service-agent   # 在 src/platform/darwin/rust workspace 内
```

产物在 `target/<profile>/exv-vpn-darwin-service-agent`；发布构建由
`scripts/build-unsigned.sh` 统一构建，`scripts/package-unsigned.sh` 把它随附进
`EXV.app/Contents/MacOS/`（四件套：tauri/core/engine/服务代理）。

## 固定用法（本文不实际执行这些命令）

（`install` / `uninstall` / `retire-legacy` / `sweep-runtime-residue` 需要管理员权限，
等价于经系统密码弹窗提权执行；`status` / `cleanup` 由普通用户发起，走本代理的控制 socket。
下例中带「（root）」注释的两条即管理员形态。）

```bash
# （root）
target/debug/exv-vpn-darwin-service-agent install
target/debug/exv-vpn-darwin-service-agent status
target/debug/exv-vpn-darwin-service-agent cleanup
# （root）
target/debug/exv-vpn-darwin-service-agent uninstall
```

提权形态（core 经 macOS 系统密码弹窗以管理员权限执行固定 payload）：弹窗的 root shell
没有 `SUDO_UID`/`SUDO_GID`，owner 由显式 `--owner-uid/--owner-gid` flags 给出并完全忽略
`SUDO_*`；信任边界是系统授权弹窗授权整条 payload 文本。下面五条都是管理员形态：

```bash
exv-vpn-darwin-service-agent install --owner-uid 501 --owner-gid 20 \
    --engine-path "/Applications/EXV.app/Contents/MacOS/exv-vpn-darwin-engine"
exv-vpn-darwin-service-agent uninstall --owner-uid 501 --owner-gid 20
exv-vpn-darwin-service-agent start
exv-vpn-darwin-service-agent start-engine-once --owner-uid 501 --owner-gid 20 \
    --runtime-dir "/private/tmp/exv-vpn-<pid>-<hex>" --core-pid <core pid> \
    --engine-path "/Applications/EXV.app/Contents/MacOS/exv-vpn-darwin-engine"
exv-vpn-darwin-service-agent retire-legacy
```

`install` 为替换式（幂等）：已安装时先验证 enrolled owner（不同用户报
`EnrolledOwnerMismatch`，防止劫持他人 daemon），best-effort `bootout`（失败忽略）后
guarded remove 旧 artifact 再重装；enrollment descriptor 缺失（干净首装或孤儿残留态）
时同一位置的回收走孤儿规则（父目录缺失=无需回收的成功；父目录受控时按「受控属主 +
固定叶名 + 非 symlink」回收）。`--engine-path` 是唯一被批准的自由文本输入：绝对路径、
长度 ≤ 1024、组件不含 `..`、常规文件且非 symlink，校验通过后原文写入 root 0600 的固定
state 叶（`/Library/Application Support/EXV/ServiceAgent/state.v1`，无换行）；daemon
serve 端读取该叶并同样校验，缺失或不合法回退**已装 Engine 固定路径**（不含任何开发
检出路径）。`start` 是 core 经 osascript 提权按需启动 daemon 的唯一入口（bootout 失败
忽略，bootstrap 失败才报错；对健康 daemon 等价于重启）。`cleanup` 仅清理本代理的
state/log；除 `--engine-path` 外 CLI 没有任意路径参数。

`retire-legacy` 是一次性迁移动词：bootout 历史/已退役 label（含旧开发伴侣 daemon）→
删除对应 plist → 递归回收历史安装目录（含 `DevCompanion/`）。目标缺席一律视为成功
（干净机不因父目录不存在而中止），逐件向 stderr 报告 `contract_key` 与维度。

## 命名空间（代码内固定常量，无 CLI 参数可覆盖）

| 产物 | 固定路径 |
| --- | --- |
| 代理二进制 | `/Library/Application Support/EXV/ServiceAgent/exv-vpn-darwin-service-agent` |
| daemon plist | `/Library/LaunchDaemons/com.exv.vpn.service-agent.plist`（label `com.exv.vpn.service-agent`） |
| engine 二进制 | `/Library/Application Support/EXV/ServiceAgent/exv-vpn-darwin-engine` |
| engine plist | `/Library/LaunchDaemons/com.exv.vpn.engine.plist`（label `com.exv.vpn.engine`） |
| 控制 socket | `/Library/Application Support/EXV/ServiceAgent/control.sock` |
| engine 端点 | `/Library/Application Support/EXV/ServiceAgent/engine.sock` |
| state / log 叶 | `…/ServiceAgent/state.v1`、`…/ServiceAgent/service.log` |

`uninstall` 回收上表全部 artifact 并**只回收空目录**（`ServiceAgent/` 与
`/Library/Application Support/EXV`；非空即保留现场、绝不递归删除）。
