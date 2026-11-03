#!/usr/bin/env bash
# MAC-PACKAGE-12：组装 `target/darwin-package/EXV.app`（**ad-hoc 签名**，非分发签名）。
#
# 输入：scripts/build-unsigned.sh 的 release 四件套（主仓 target/release：tauri/core/
# engine 三件套 + 服务代理 exv-vpn-darwin-service-agent）与前端静态文件（前端已由
# Tauri generate_context! 编译期嵌入壳 binary，不再单独放置）。
#
# bundle 布局（Engine 位于固定 bundle path；壳/Core 按同目录 basename 解析；服务代理
# 供 core 经 osascript 提权安装/启动服务 daemon）：
#   EXV.app/
#     Contents/
#       Info.plist                       （最小合法 plist；版本读自 tauri.conf.json）
#       PkgInfo
#       MacOS/exv-vpn-darwin-{tauri,core,engine}
#       MacOS/exv-vpn-darwin-service-agent
#
# 产物纪律：打包前精确删除主仓 target/darwin-package 顶层旧 EXV 产物（*.app/*.dmg/*.pkg），
# 临时组装目录收尾清除；组装目录最多保留一个当前 `EXV.app`。
# 组装完成后做 **ad-hoc 签名**（`codesign --sign -`）：macOS 26 起系统通知（本壳
# notification_modern 的 UNUserNotificationCenter 后端）要求 app bundle 至少带 ad-hoc
# 签名——未签名时通知授权直接回 UNErrorDomain error 1（notifications not allowed），
# 签名后正常弹授权并投递（2026-09-20 真机实证）。
# ad-hoc 签名**不是分发签名**：不改变 Gatekeeper/公证状态，卷内「提示已损坏」说明与
# 隔离标记处置保持不变。
# 不生成 appex/dmg/pkg/versioned archive，不做分发签名、不公证。
set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
readonly CHECKOUT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
python3 "${SCRIPT_DIR}/release.py" --root "${CHECKOUT_ROOT}" version check --notes
readonly MAIN_REPO_ROOT="$(cd -- "$(git -C "${CHECKOUT_ROOT}" rev-parse --path-format=absolute --git-common-dir)/.." && pwd -P)"

readonly SHARED_TARGET_DIR="${MAIN_REPO_ROOT}/target"
readonly DIST_DIR="${SHARED_TARGET_DIR}/darwin-package"
readonly APP_NAME="EXV.app"
readonly APP_BUNDLE="${DIST_DIR}/${APP_NAME}"
readonly TAURI_APP_CONFIG="${CHECKOUT_ROOT}/src/platform/darwin/rust/tauri/app/tauri.conf.json"
readonly BUNDLE_ID="com.exv.vpn.desktop"
readonly BINARIES=(
    "exv-vpn-darwin-tauri"
    "exv-vpn-darwin-core"
    "exv-vpn-darwin-engine"
    "exv-vpn-darwin-service-agent"
)

log() { printf '[package-unsigned] %s\n' "$*"; }
fail() { printf '[package-unsigned] 失败：%s\n' "$*" >&2; exit 1; }

# ---- 前置校验 ---------------------------------------------------------------

command -v node >/dev/null 2>&1 || fail "需要 node 读取产品版本"

command -v plutil >/dev/null 2>&1 || fail "需要 macOS 自带 plutil"
for binary in "${BINARIES[@]}"; do
    [[ -x "${SHARED_TARGET_DIR}/release/${binary}" ]] \
        || fail "缺 release 产物 ${binary}；先运行 scripts/build-unsigned.sh"
done

# 产品版本从 Tauri 配置读取；Cargo crate 版本不代表发布版本。
readonly PRODUCT_VERSION="$(node -e 'process.stdout.write(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).version)' "${TAURI_APP_CONFIG}")"
[[ "${PRODUCT_VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "无法从 ${TAURI_APP_CONFIG} 读取语义化版本"

# ---- 产物纪律：精确删除 组装目录顶层旧 EXV 产物 --------------------------------

mkdir -p -- "${DIST_DIR}"
log "清理 组装目录顶层旧产物（*.app / *.dmg / *.pkg，精确单层）"
shopt -s nullglob
for stale in "${DIST_DIR}"/*.app "${DIST_DIR}"/*.dmg "${DIST_DIR}"/*.pkg; do
    rm -rf -- "${stale}"
    log "  已删除 ${stale}"
done
# 打包中断（含 kill -9 跳过 trap）可能遗留临时组装目录；一并精确清扫，
# 保证 组装目录内除本次 staging 外无任何中间产物。
for stale in "${DIST_DIR}/.exv-pkg-staging."*; do
    rm -rf -- "${stale}"
    log "  已删除中断残留 ${stale}"
done
shopt -u nullglob

# ---- 组装（临时目录 → 原子改名）---------------------------------------------

readonly STAGING="${DIST_DIR}/.exv-pkg-staging.$$"
cleanup_staging() { rm -rf -- "${STAGING}"; }
trap cleanup_staging EXIT

log "组装 ${APP_BUNDLE}（version ${PRODUCT_VERSION}）"
mkdir -p -- "${STAGING}/Contents/MacOS" "${STAGING}/Contents/Resources"

for binary in "${BINARIES[@]}"; do
    cp -p -- "${SHARED_TARGET_DIR}/release/${binary}" "${STAGING}/Contents/MacOS/${binary}"
    chmod 0755 "${STAGING}/Contents/MacOS/${binary}"
done

readonly APP_ICON="${CHECKOUT_ROOT}/src/platform/darwin/rust/tauri/app/icons/AppIcon.icns"
[[ -f "${APP_ICON}" ]] || fail "缺应用图标 ${APP_ICON}"
cp -p -- "${APP_ICON}" "${STAGING}/Contents/Resources/AppIcon.icns"

cat >"${STAGING}/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>CFBundleName</key>
	<string>EXV</string>
	<key>CFBundleDisplayName</key>
	<string>EXV</string>
	<key>CFBundleIdentifier</key>
	<string>${BUNDLE_ID}</string>
	<key>CFBundleExecutable</key>
	<string>exv-vpn-darwin-tauri</string>
	<key>CFBundleShortVersionString</key>
	<string>${PRODUCT_VERSION}</string>
	<key>CFBundleVersion</key>
	<string>${PRODUCT_VERSION}</string>
	<key>CFBundlePackageType</key>
	<string>APPL</string>
	<key>CFBundleIconFile</key>
	<string>AppIcon</string>
	<key>CFBundleIconName</key>
	<string>AppIcon</string>
	<key>LSMinimumSystemVersion</key>
	<string>11.0</string>
	<key>NSHighResolutionCapable</key>
	<true/>
	<key>NSPrincipalClass</key>
	<string>NSApplication</string>
</dict>
</plist>
PLIST

printf 'APPL????' >"${STAGING}/Contents/PkgInfo"

plutil -lint -- "${STAGING}/Contents/Info.plist" >/dev/null \
    || fail "生成的 Info.plist 未通过 plutil -lint"

# 原子替换：同层改名，失败不留半成品 .app。
mv -- "${STAGING}" "${APP_BUNDLE}"
trap - EXIT

# ---- ad-hoc 签名 ------------------------------------------------------------

log "ad-hoc 签名（codesign --sign -，非分发签名）"
/usr/bin/codesign --force --sign - "${APP_BUNDLE}" || fail "ad-hoc 签名失败"
/usr/bin/codesign --verify --strict "${APP_BUNDLE}" || fail "ad-hoc 签名校验失败"
printf '  %s\n' "$(/usr/bin/codesign -dv "${APP_BUNDLE}" 2>&1 | grep -E 'Identifier|Signature' | tr '\n' ' ')"

# ---- 产出校验 ---------------------------------------------------------------

log "打包完成；组装目录当前内容"
find "${DIST_DIR}" -maxdepth 1 -mindepth 1 | sort | while IFS= read -r entry; do
    printf '  %s\n' "$(basename -- "${entry}")"
done
log "bundle 清单"
find "${APP_BUNDLE}" -print | sort | sed 's/^/  /'
# 注意：下面的哈希是**签名后**bundle 内的哈希——主可执行文件（tauri）会被 codesign 重写
# （嵌入/更新签名），因此它与构建输出不再逐字一致；三个辅助二进制不变（由 CodeResources
# 以哈希封闭）。核验口径：辅助三件与构建输出逐字比对 + 主程序 codesign --verify --strict。
log "四件套 sha256（签名后；主可执行文件含签名，与构建输出不同属预期）"
for binary in "${BINARIES[@]}"; do
    printf '  %s  %s\n' "${binary}" "$(shasum -a 256 -- "${APP_BUNDLE}/Contents/MacOS/${binary}" | awk '{print $1}')"
done
log "ad-hoc 签名产物就绪：${APP_BUNDLE}"
