#!/usr/bin/env bash
# Darwin DMG 流水线阶段：输入 package-unsigned.sh 生成的 EXV.app。
# 统一入口须在构建前执行全仓清理；此阶段不重复清理其依赖的构建输入。
# 最终镜像进入主仓同级共享产物目录；临时素材随退出清除。
set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
readonly CHECKOUT_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"
python3 "${SCRIPT_DIR}/release.py" --root "${CHECKOUT_ROOT}" version check --notes

readonly MAIN_REPO_ROOT="$(cd -- "$(git -C "${CHECKOUT_ROOT}" rev-parse --path-format=absolute --git-common-dir)/.." && pwd -P)"
readonly DIST_DIR="${MAIN_REPO_ROOT}/../exv-artifacts"
mkdir -p -- "${DIST_DIR}"
readonly APP_NAME="EXV.app"
readonly APP_BUNDLE="${EXV_DMG_APP_BUNDLE:-${MAIN_REPO_ROOT}/target/darwin-package/${APP_NAME}}"
readonly TAURI_APP_CONFIG="${CHECKOUT_ROOT}/src/platform/darwin/rust/tauri/app/tauri.conf.json"
readonly VOLUME_NAME="安装EXV"
readonly BUNDLE_ID="com.exv.vpn.desktop"
readonly WINDOW_LEFT=100
readonly WINDOW_TOP=100
readonly WINDOW_WIDTH=760
readonly WINDOW_HEIGHT=600
readonly ICON_SIZE=96
readonly TEXT_SIZE=12
readonly APP_ICON_X=190
readonly APP_ICON_Y=140
readonly APPS_ICON_X=570
readonly APPS_ICON_Y=140
readonly CMD_ICON_X=190
readonly CMD_ICON_Y=325
readonly GUIDE_ICON_X=570
readonly GUIDE_ICON_Y=325

# dmgbuild 与回读断言所需的 Python 解释器：默认系统自带 python3（依赖装在 user
# site-packages）。可用 EXV_DMG_PYTHON 覆盖。
readonly PYTHON_BIN="${EXV_DMG_PYTHON:-/usr/bin/python3}"

declare -a MOUNT_POINTS=()
declare STAGING=""

log() { printf '[package-dmg] %s\n' "$*"; }
fail() { printf '[package-dmg] 失败：%s\n' "$*" >&2; exit 1; }

cleanup() {
    local mount
    for mount in "${MOUNT_POINTS[@]:-}"; do
        [[ -n "${mount}" && -d "${mount}" ]] && hdiutil detach "${mount}" -quiet 2>/dev/null || true
    done
    [[ -n "${STAGING}" && -d "${STAGING}" ]] && rm -rf -- "${STAGING}"
    return 0
}
trap cleanup EXIT

# ---- 前置校验：命令与 Python 依赖（fail-fast，不自动安装）-------------------

command -v hdiutil >/dev/null 2>&1 || fail "需要 macOS 自带 hdiutil"
[[ -x "${PYTHON_BIN}" ]] || fail "缺 Python 解释器 ${PYTHON_BIN}；可用 EXV_DMG_PYTHON 指定"

readonly PIP_HINT="${PYTHON_BIN} -m pip install --user dmgbuild Pillow ds_store mac_alias"
if ! MISSING="$("${PYTHON_BIN}" - <<'PY'
import importlib
import sys

missing = []
for module in ("dmgbuild", "PIL", "ds_store", "mac_alias"):
    try:
        importlib.import_module(module)
    except Exception:
        missing.append(module)
if missing:
    sys.stdout.write(", ".join(missing))
    raise SystemExit(1)
PY
)"; then
    fail "Python 依赖缺失：${MISSING}。请在本机安装后重试（本脚本不自动安装）：
    ${PIP_HINT}"
fi
[[ -d "${APP_BUNDLE}" ]] || fail "缺 ${APP_BUNDLE}；先运行 scripts/build-unsigned.sh 与 scripts/package-unsigned.sh"
[[ -x "${APP_BUNDLE}/Contents/MacOS/exv-vpn-darwin-tauri" ]] || fail "bundle 内缺可执行外壳"


readonly PRODUCT_VERSION="$(node -e 'process.stdout.write(JSON.parse(require("fs").readFileSync(process.argv[1], "utf8")).version)' "${TAURI_APP_CONFIG}")"
[[ "${PRODUCT_VERSION}" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || fail "无法从 ${TAURI_APP_CONFIG} 读取语义化版本"
readonly OUTPUT_DMG="${DIST_DIR}/安装EXV.dmg"

# ---- 产物纪律：精确删除旧的 dmg 与中断残留 ----------------------------------

shopt -s nullglob
for stale in "${DIST_DIR}"/*.dmg; do
    rm -f -- "${stale}"
    log "已删除旧产物 ${stale}"
done
# 本脚本 staging、dmgbuild 自己的中间镜像（`.temp*` 前缀）与旧 AppleScript 方案的
# 读写镜像（`.exv-dmg-rw.*`）一并精确清扫。
for stale in "${DIST_DIR}/.exv-dmg-staging."* "${DIST_DIR}/.exv-dmg-rw."* "${DIST_DIR}/.temp"*.dmg; do
    rm -rf -- "${stale}"
    log "已删除中断残留 ${stale}"
done
shopt -u nullglob

# ---- staging：背景图、安装说明、修复脚本 ------------------------------------

STAGING="$(mktemp -d "${DIST_DIR}/.exv-dmg-staging.XXXXXX")"
log "生成卷内容素材（version ${PRODUCT_VERSION}）"

"${PYTHON_BIN}" "${SCRIPT_DIR}/darwin_dmg_assets.py" \
    --output "${STAGING}" \
    --app-icon "${CHECKOUT_ROOT}/src/platform/darwin/rust/tauri/app/icons/AppIcon.icns"
readonly VOLUME_ICON="${STAGING}/InstallerIcon.icns"

# ---- 残留卷竞态：调用 dmgbuild 前先 detach 同名残留卷 ------------------------

# 枚举所有以卷名开头的挂载点：既包含 `/Volumes/EXV`，也包含系统改名产生的
# `/Volumes/EXV 1`、`/Volumes/EXV 2`（上次异常中断留下的）。不清干净就拿不到干净卷名。
shopt -s nullglob
for stale in "/Volumes/${VOLUME_NAME}" "/Volumes/${VOLUME_NAME} "[0-9]*; do
    [[ -d "${stale}" ]] || continue
    log "发现残留挂载卷 ${stale}（上次异常中断），先卸载"
    hdiutil detach "${stale}" -force -quiet \
        || fail "无法卸载残留卷 ${stale}；请手动执行 hdiutil detach '${stale}'"
done
shopt -u nullglob

# ---- dmgbuild：组装 + 写 .DS_Store + 转 UDZO --------------------------------

log "用 dmgbuild 生成 ${OUTPUT_DMG}（图标 ${ICON_SIZE}，窗口 ${WINDOW_WIDTH}×${WINDOW_HEIGHT}）"
"${PYTHON_BIN}" - \
    "${APP_BUNDLE}" \
    "${STAGING}/安装说明.txt" \
    "${STAGING}/提示已损坏时运行.command" \
    "${STAGING}/bg.png" \
    "${VOLUME_ICON}" \
    "${OUTPUT_DMG}" \
    "${VOLUME_NAME}" \
    "${SCRIPT_DIR}" \
    "${WINDOW_LEFT}" "${WINDOW_TOP}" "${WINDOW_WIDTH}" "${WINDOW_HEIGHT}" \
    "${ICON_SIZE}" "${TEXT_SIZE}" \
    "${APP_ICON_X}" "${APP_ICON_Y}" \
    "${APPS_ICON_X}" "${APPS_ICON_Y}" \
    "${CMD_ICON_X}" "${CMD_ICON_Y}" \
    "${GUIDE_ICON_X}" "${GUIDE_ICON_Y}" <<'PY'
"""调用 dmgbuild 生成 DMG（替代旧 AppleScript 布局路线）。

dmgbuild 内部流程：hdiutil create(UDRW) → attach → 复制 files/symlinks → 写
`.DS_Store`（icvp/Iloc/背景 alias）→ detach → resize → hdiutil convert(UDZO)。
"""
import sys

import dmgbuild

(
    app_bundle,
    guide_txt,
    repair_command,
    background,
    volume_icon,
    output_dmg,
    volume_name,
    script_dir,
    window_left,
    window_top,
    window_width,
    window_height,
    icon_size,
    text_size,
    app_x,
    app_y,
    apps_x,
    apps_y,
    cmd_x,
    cmd_y,
    guide_x,
    guide_y,
) = sys.argv[1:]

window_left, window_top = int(window_left), int(window_top)
window_width, window_height = int(window_width), int(window_height)
icon_size, text_size = float(icon_size), float(text_size)

settings = {
    "files": [app_bundle, guide_txt, repair_command],
    "symlinks": {"Applications": "/Applications"},
    "background": background,
    # 卷图标 = 安装专用图标（dmgbuild → `.VolumeIcon.icns` + `SetFile -a C`）。
    "icon": volume_icon,
    "window_rect": ((window_left, window_top), (window_width, window_height)),
    "icon_size": icon_size,
    "text_size": text_size,
    # `None` → arrangeBy "none"（保持手摆位置，不自动排列）。
    "arrange_by": None,
    "show_status_bar": False,
    "show_tab_view": False,
    "show_toolbar": False,
    "show_pathbar": False,
    "show_sidebar": False,
    # 明确写入顶端起始位置；不依赖 dmgbuild 的默认值，避免已有 Finder 状态下
    # 生成的布局从非零位置开始。
    "scroll_position": (0.0, 0.0),
    "icon_locations": {
        "EXV.app": (int(app_x), int(app_y)),
        "Applications": (int(apps_x), int(apps_y)),
        "提示已损坏时运行.command": (int(cmd_x), int(cmd_y)),
        "安装说明.txt": (int(guide_x), int(guide_y)),
    },
    "format": "UDZO",
    "compression_level": 9,
}

# 注意：dmgbuild 把背景复制到卷根 `.background.png`（背景 alias 指向该路径），
# 而不是计划文字里的 `.background/bg.png`；`backgroundType: 2` + alias 语义一致。
settings["filename"] = output_dmg
settings["volume_name"] = volume_name


def progress(info):
    kind = info.get("type", "")
    if kind == "operation::finished" and info.get("operation") == "dsstore::create":
        sys.path.insert(0, script_dir)
        from darwin_dmg_bookmark import repair_background
        repair_background("/Volumes/" + volume_name)
    if kind in ("operation::start", "command::start"):
        print(
            f"[dmgbuild] {kind}: {info.get('operation') or info.get('command')}",
            file=sys.stderr,
        )


dmgbuild.build_dmg(
    output_dmg,
    volume_name,
    settings=settings,
    callback=progress,
)
print("[dmgbuild] 完成", file=sys.stderr)
PY

log "dmgbuild 构建日志（尾部）已随上述输出打印"
[[ -s "${OUTPUT_DMG}" ]] || fail "dmgbuild 未产出 ${OUTPUT_DMG}"

# ---- 产出校验 ---------------------------------------------------------------

# 文件图标与挂载卷图标分别设置：用户在 Finder 中看到的 .dmg 也带安装徽标。
osascript -l JavaScript - "${VOLUME_ICON}" "${OUTPUT_DMG}" <<'JXA'
ObjC.import('AppKit');
function run(argv) {
    const icon = $.NSImage.alloc.initWithContentsOfFile(argv[0]);
    if (!$.NSWorkspace.sharedWorkspace.setIconForFileOptions(icon, argv[1], 0)) {
        throw new Error('无法设置 DMG 文件安装图标');
    }
}
JXA

log "校验镜像"
hdiutil verify "${OUTPUT_DMG}" >/dev/null 2>&1 || fail "hdiutil verify 未通过"

log "只读挂载复核：卷内容 + .DS_Store 回读断言"
VERIFY_OUTPUT="$(hdiutil attach "${OUTPUT_DMG}" -readonly -noverify -noautoopen 2>&1)" \
    || fail "只读复核挂载失败"
VERIFY_MOUNT="$(printf '%s\n' "${VERIFY_OUTPUT}" | awk -F'\t' '/\/Volumes\//{print $NF; exit}')"
[[ -n "${VERIFY_MOUNT}" && -d "${VERIFY_MOUNT}" ]] || fail "无法解析复核挂载点"
# 挂载点必须正好是预期卷名：若被系统改名（`EXV-123 1`），回读的就是别的卷。
[[ "$(basename -- "${VERIFY_MOUNT}")" == "${VOLUME_NAME}" ]] \
    || fail "复核挂载点为 ${VERIFY_MOUNT}，与预期卷名 ${VOLUME_NAME} 不符（疑似残留卷未清理）；已中止"
MOUNT_POINTS+=("${VERIFY_MOUNT}")
find "${VERIFY_MOUNT}" -maxdepth 1 -mindepth 1 | sort | sed 's/^/  /'
[[ -d "${VERIFY_MOUNT}/${APP_NAME}" ]] || fail "卷内缺 ${APP_NAME}"
[[ -L "${VERIFY_MOUNT}/Applications" ]] || fail "卷内缺 Applications 符号链接"
[[ -f "${VERIFY_MOUNT}/安装说明.txt" ]] || fail "卷内缺 安装说明.txt"
[[ -x "${VERIFY_MOUNT}/提示已损坏时运行.command" ]] || fail "提示已损坏时运行.command 不可执行"
[[ -f "${VERIFY_MOUNT}/.DS_Store" ]] || fail "卷内缺 .DS_Store（布局未写入）"
# 卷图标断言：图标文件必须在卷内，且卷已带自定义图标位（`SetFile -a C` 的效果）。
[[ -f "${VERIFY_MOUNT}/.VolumeIcon.icns" ]] || fail "卷内缺 .VolumeIcon.icns（卷图标未写入）"
cmp -s "${VOLUME_ICON}" "${VERIFY_MOUNT}/.VolumeIcon.icns" \
    || fail "卷图标与安装图标不一致"
[[ "$(/usr/bin/GetFileInfo -a "${VERIFY_MOUNT}" | tr -d ' ')" == *C* ]] \
    || fail "卷未设置自定义图标位（SetFile -a C 未生效）"

# 回读断言：必须解析 `hdiutil convert` 之后的**最终只读镜像**上的 `.DS_Store`
# （不是 Finder 回读：Finder 会按自己的默认值重写 `.DS_Store`，实测把 128 覆写回 96）。
"${PYTHON_BIN}" - "${VERIFY_MOUNT}/.DS_Store" \
    "${ICON_SIZE}" "${TEXT_SIZE}" \
    "${APP_ICON_X}" "${APP_ICON_Y}" \
    "${APPS_ICON_X}" "${APPS_ICON_Y}" \
    "${CMD_ICON_X}" "${CMD_ICON_Y}" \
    "${GUIDE_ICON_X}" "${GUIDE_ICON_Y}" <<'PY'
"""解析最终镜像的 `.DS_Store`，断言图标尺寸与已定位图标坐标；不通过即 fail。"""
import sys

from ds_store import DSStore

(
    ds_store_path,
    icon_size,
    text_size,
    app_x,
    app_y,
    apps_x,
    apps_y,
    cmd_x,
    cmd_y,
    guide_x,
    guide_y,
) = sys.argv[1:]
expected_icon_size = float(icon_size)
expected_text_size = float(text_size)
expected_locations = {
    "EXV.app": (int(app_x), int(app_y)),
    "Applications": (int(apps_x), int(apps_y)),
    "提示已损坏时运行.command": (int(cmd_x), int(cmd_y)),
    "安装说明.txt": (int(guide_x), int(guide_y)),
}

failures = []
with DSStore.open(ds_store_path, "r") as store:
    icvp = store["."]["icvp"]
    actual_icon_size = float(icvp["iconSize"])
    actual_text_size = float(icvp["textSize"])
    actual_arrange = icvp["arrangeBy"]
    actual_background_type = icvp["backgroundType"]
    has_background_alias = "backgroundImageAlias" in icvp

    print(f"[readback] iconSize={actual_icon_size} textSize={actual_text_size}")
    print(
        f"[readback] arrangeBy={actual_arrange} backgroundType={actual_background_type} "
        f"backgroundImageAlias={has_background_alias}"
    )

    if actual_icon_size != expected_icon_size:
        failures.append(f"iconSize={actual_icon_size}，期望 {expected_icon_size}")
    if actual_text_size != expected_text_size:
        failures.append(f"textSize={actual_text_size}，期望 {expected_text_size}")
    if actual_arrange != "none":
        failures.append(f"arrangeBy={actual_arrange!r}，期望 'none'（不自动排列）")
    if actual_background_type != 2:
        failures.append(f"backgroundType={actual_background_type}，期望 2（自定义图片）")
    if not has_background_alias:
        failures.append("icvp 缺 backgroundImageAlias（背景 alias 未写入）")

    for name, expected in expected_locations.items():
        actual = tuple(int(value) for value in store[name]["Iloc"])
        print(f"[readback] Iloc[{name}]={actual}")
        if actual != expected:
            failures.append(f"Iloc[{name}]={actual}，期望 {expected}")

if failures:
    for failure in failures:
        print(f"[readback] 断言失败：{failure}", file=sys.stderr)
    raise SystemExit(1)
print("[readback] .DS_Store 断言全部通过")
PY

"${PYTHON_BIN}" - "${SCRIPT_DIR}" "${VERIFY_MOUNT}" <<'PYBOOKMARK'
import sys
sys.path.insert(0, sys.argv[1])
from darwin_dmg_bookmark import verify_background
verify_background(sys.argv[2])
PYBOOKMARK

hdiutil detach "${VERIFY_MOUNT}" -quiet || fail "复核卸载失败"
MOUNT_POINTS=()

readonly DMG_SHA="$(shasum -a 256 -- "${OUTPUT_DMG}" | awk '{print $1}')"
readonly DMG_SIZE="$(du -h -- "${OUTPUT_DMG}" | awk '{print $1}')"
log "打包完成：${OUTPUT_DMG}"
log "  version=${PRODUCT_VERSION}  size=${DMG_SIZE}  sha256=${DMG_SHA}"
log "  bundle id=${BUNDLE_ID}（ad-hoc 签名、未公证；遇到打开提示请按卷内说明处理）"
