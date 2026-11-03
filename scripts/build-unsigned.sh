#!/usr/bin/env bash
# MAC-PACKAGE-12：无签名 `EXV VPN.app` 的构建脚本。
#
# 产出当前产品的 release 四件套（tauri/core/engine + 服务代理）与前端静态文件：
#   * 前端产物固定输出到主仓 `target/darwin-tauri-frontend`（与
#     `tauri/app/tauri.conf.json` 的 `build.frontendDist` 逐字一致，构建前校验）；
#   * 四件套经共享主仓 `target/`（显式 CARGO_TARGET_DIR）以 `--locked` 构建；服务代理
#     是产品 workspace 成员（crates/exv-vpn-darwin-service-agent），与其余产品 crate
#     同一构建；
#   * 产物纪律：入口先调用全仓构建产物清理脚本，后续打包阶段沿用本次输入。
# 不做签名、公证或任何 bundle 打包（打包见 scripts/package-unsigned.sh）。
set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
# 当前 checkout 根（可能是主仓，也可能是仓内 .worktrees/ 下的工作树）。
readonly CHECKOUT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
python3 "${SCRIPT_DIR}/release.py" --root "${CHECKOUT_ROOT}" version check --notes
# 主仓根：worktree 的 `.git` 是指向主仓 gitdir 的文件/链接，git-common-dir 永远
# 指回主仓；共享产物唯一位置（主仓 target/）由此推导。
readonly MAIN_REPO_ROOT="$(cd -- "$(git -C "${CHECKOUT_ROOT}" rev-parse --path-format=absolute --git-common-dir)/.." && pwd -P)"

readonly DARWIN_RUST_DIR="${CHECKOUT_ROOT}/src/platform/darwin/rust"
readonly FRONTEND_DIR="${DARWIN_RUST_DIR}/tauri/frontend"
readonly TAURI_CONF="${DARWIN_RUST_DIR}/tauri/app/tauri.conf.json"
readonly SHARED_TARGET_DIR="${MAIN_REPO_ROOT}/target"
readonly FRONTEND_DIST="${SHARED_TARGET_DIR}/darwin-tauri-frontend"
readonly BINARIES=(
    "exv-vpn-darwin-tauri"
    "exv-vpn-darwin-core"
    "exv-vpn-darwin-engine"
    "exv-vpn-darwin-service-agent"
)

log() { printf '[build-unsigned] %s\n' "$*"; }
fail() { printf '[build-unsigned] 失败：%s\n' "$*" >&2; exit 1; }

# ---- 前置校验 ---------------------------------------------------------------

command -v cargo >/dev/null 2>&1 || fail "需要全局 cargo（Rust 工具链）"
command -v sccache >/dev/null 2>&1 || fail "需要 sccache 编译缓存"
export RUSTC_WRAPPER=sccache
export CARGO_ENCODED_RUSTFLAGS="$(python3 "${SCRIPT_DIR}/release.py" --root "${CHECKOUT_ROOT}" build-flags)"
command -v node >/dev/null 2>&1 || fail "需要全局 node"
command -v npm >/dev/null 2>&1 || fail "需要全局 npm"
[[ -f "${DARWIN_RUST_DIR}/Cargo.toml" ]] || fail "缺 ${DARWIN_RUST_DIR}/Cargo.toml"
[[ -f "${FRONTEND_DIR}/package.json" ]] || fail "缺 ${FRONTEND_DIR}/package.json"
[[ -f "${TAURI_CONF}" ]] || fail "缺 ${TAURI_CONF}"

# frontendDist 防漂移：配置内声明的路径必须等于本脚本的输出位置。
CONF_FRONTEND_DIST="$(node -e 'const p=require("path");process.stdout.write(p.resolve(p.dirname(process.argv[1]),JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).build.frontendDist))' "${TAURI_CONF}")"
[[ "${CONF_FRONTEND_DIST}" == "${FRONTEND_DIST}" ]] || fail "tauri.conf.json build.frontendDist（${CONF_FRONTEND_DIST}）与脚本输出位置（${FRONTEND_DIST}）不一致，拒绝构建"

# ---- 产物纪律：整条构建流水线入口统一清理 --------------------------------

bash "${SCRIPT_DIR}/clear-repository-build-artifacts.sh" --keep-latest-installer
# Apple Silicon 产品最低系统版本与 package-unsigned.sh 的 Info.plist 一致。
export MACOSX_DEPLOYMENT_TARGET=11.0

# ---- 前端静态文件 -----------------------------------------------------------

log "构建前端（输出 ${FRONTEND_DIST}）"
npm --prefix "${CHECKOUT_ROOT}/src/platform/win32/rust/tauri/frontend" ci --prefer-offline --no-audit --no-fund
npm --prefix "${FRONTEND_DIR}" ci --prefer-offline --no-audit --no-fund
npm --prefix "${FRONTEND_DIR}" run build -- --outDir "${FRONTEND_DIST}" --emptyOutDir
[[ -f "${FRONTEND_DIST}/index.html" ]] || fail "前端产物缺 index.html"

# ---- release 四件套（共享主仓 target，--locked，分段构建）------------------

cargo_cmd() {
    CARGO_TARGET_DIR="${SHARED_TARGET_DIR}" \
        cargo build --locked --release --manifest-path "${DARWIN_RUST_DIR}/Cargo.toml" "$@"
}

log "构建 release core（exv-vpn-darwin-core）"
cargo_cmd -p exv-vpn-darwin-core

log "构建 release engine（exv-vpn-darwin-engine）"
cargo_cmd -p exv-vpn-darwin-engine

# Tauri 的 generate_context! 在编译期把 FRONTEND_DIST 嵌入 release binary，
# 因此必须在前端产物就绪后构建。
log "构建 release tauri 壳（exv-vpn-darwin-tauri）"
cargo_cmd -p exv-vpn-darwin-tauri

log "构建 release 服务代理（exv-vpn-darwin-service-agent）"
cargo_cmd -p exv-vpn-darwin-service-agent

# ---- 产出校验 ---------------------------------------------------------------

log "本次构建产物清单"
for binary in "${BINARIES[@]}"; do
    artifact="${SHARED_TARGET_DIR}/release/${binary}"
    [[ -f "${artifact}" && -x "${artifact}" ]] || fail "构建后缺可执行产物 ${artifact}"
    printf '  %s\n     sha256=%s\n' "${artifact}" "$(shasum -a 256 -- "${artifact}" | awk '{print $1}')"
done

log "构建完成：四件套 + 前端静态文件就绪（未打包；运行 scripts/package-unsigned.sh 组装主仓 target/darwin-package/EXV.app）"
