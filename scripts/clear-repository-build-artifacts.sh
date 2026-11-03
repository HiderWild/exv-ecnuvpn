#!/usr/bin/env bash
# 构建产物清理（darwin/POSIX 宿主），与 scripts/clear-repository-build-artifacts.ps1
# 同语义；路径安全以 realpath 解析为准，未逐项对齐 ps1 的 reparse/symlink 检查
# （ps1 另拒绝符号链接祖先与 symlink 工作树根；本脚本依赖 realpath + path_within
# 覆盖「逃逸到工作树外」这一核心场景，树内 symlink 祖先按解析后目标删除）。
# ps1 未覆盖的 darwin 嵌套产物位置在此补齐。
#
# 纪律（见 AGENTS.md「构建产物共享与清理纪律」）：
#   * 每次构建或打包前必须执行；验证一经通过必须立即再次执行，产物不得囤积，
#     重建依赖 sccache 缓存；
#   * 枚举主仓与全部已注册工作树，无条件清除其中的构建目录、前端产物、
#     Rust/Tauri target 与历史安装包副本；
#   * 共享产物根固定为主仓同级 `exv-artifacts`，必须位于所有
#     工作树之外；与 ps1 版一致，不接受自定义覆盖；
#   * `--keep-latest-installer`：把最新一份有效安装包（Windows 校验 MZ 头，Darwin
#     镜像经 hdiutil verify；复制后 sha256 复核）收敛到共享根后，删除其余副本与暂存/回滚残片。
#
# 用法：scripts/clear-repository-build-artifacts.sh [--keep-latest-installer] [--whatif]
set -euo pipefail

readonly SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
readonly REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd -P)"

log() { printf '[clear-build-artifacts] %s\n' "$*"; }
fail() { printf '[clear-build-artifacts] 失败：%s\n' "$*" >&2; exit 1; }

command -v git >/dev/null 2>&1 || fail "需要 git"
command -v python3 >/dev/null 2>&1 || fail "需要 python3（路径规范化）"
command -v shasum >/dev/null 2>&1 || fail "需要 shasum"

KEEP_LATEST_INSTALLER=0
WHATIF=0
while [ $# -gt 0 ]; do
    case "$1" in
        --keep-latest-installer) KEEP_LATEST_INSTALLER=1 ;;
        --whatif) WHATIF=1 ;;
        *) fail "未知参数：$1（仅支持 --keep-latest-installer / --whatif）" ;;
    esac
    shift
done

# 规范化可能不存在的路径（macOS realpath 对不存在路径不可靠，借 python3）。
canon() {
    python3 -c 'import os,sys; sys.stdout.write(os.path.realpath(sys.argv[1]))' "$1"
}

path_within() { # <child> <parent>：child 是否严格位于 parent 之内（不含相等）
    local child parent
    child="$(canon "$1")"
    parent="$(canon "$2")"
    parent="${parent%/}/"
    [ "${child#"${parent}"}" != "${child}" ]
}

# 主仓根：worktree 的 .git 指向主仓 gitdir，git-common-dir 永远指回主仓
# （与 scripts/build-unsigned.sh 的推导一致）。
GIT_COMMON_DIR="$(git -C "${REPO_ROOT}" rev-parse --path-format=absolute --git-common-dir)" \
    || fail "无法解析 git common dir：${REPO_ROOT}"
readonly MAIN_REPO_ROOT="$(cd -- "$(dirname -- "${GIT_COMMON_DIR}")" && pwd -P)"
readonly SHARED_ARTIFACT_ROOT="$(cd -- "$(dirname -- "${MAIN_REPO_ROOT}")" && pwd -P)/exv-artifacts"

WORKTREES=()
while IFS= read -r line; do
    case "${line}" in
        'worktree '*) WORKTREES+=("${line#worktree }") ;;
    esac
done < <(git -C "${REPO_ROOT}" worktree list --porcelain)
if [ "${#WORKTREES[@]}" -eq 0 ]; then fail "git 未返回任何已注册工作树"; fi

for worktree in "${WORKTREES[@]}"; do
    if [ "$(canon "${SHARED_ARTIFACT_ROOT}")" = "$(canon "${worktree}")" ] \
        || path_within "${SHARED_ARTIFACT_ROOT}" "${worktree}"; then
        fail "共享产物根必须位于所有工作树之外：${SHARED_ARTIFACT_ROOT}"
    fi
done

assert_artifact_path() { # <path> <worktree>：拒绝工作树外、工作树根与符号链接
    local resolved
    resolved="$(canon "$1")"
    path_within "${resolved}" "$2" || fail "拒绝清理工作树之外的路径：${resolved}"
    [ "${resolved}" != "$(canon "$2")" ] || fail "拒绝清理工作树根目录：${resolved}"
    [ ! -L "$1" ] || fail "拒绝清理符号链接：${resolved}"
    printf '%s\n' "${resolved}"
}

readonly NESTED_OUTPUTS=(
    'src/platform/win32/rust/target'
    'src/platform/win32/rust/tauri/target'
    'src/platform/win32/rust/tauri/frontend/dist'
    'src/platform/darwin/rust/target'
    'src/platform/darwin/rust/tauri/target'
    'src/platform/darwin/rust/tauri/frontend/dist'
)
# Codex 宿主构建可能在主仓根遗留这些缓存目录，属构建产物，同样无条件清理。
readonly CODEX_RESIDUE_DIRS=(.codex-cargo-target .codex-npm-cache .codex-tmp)

ARTIFACT_DIRS=()
INSTALLER_FILES=()

for worktree in "${WORKTREES[@]}"; do
    if [ ! -d "${worktree}" ]; then continue; fi
    # 顶层构建目录：target / dist / out / build*
    for entry in "${worktree}/target" "${worktree}/dist" "${worktree}/out"; do
        if [ -d "${entry}" ]; then
            ARTIFACT_DIRS+=("$(assert_artifact_path "${entry}" "${worktree}")")
        fi
    done
    shopt -s nullglob
    for entry in "${worktree}"/build*; do
        if [ -d "${entry}" ]; then
            ARTIFACT_DIRS+=("$(assert_artifact_path "${entry}" "${worktree}")")
        fi
    done
    shopt -u nullglob
    for rel in "${NESTED_OUTPUTS[@]}"; do
        if [ -e "${worktree}/${rel}" ]; then
            ARTIFACT_DIRS+=("$(assert_artifact_path "${worktree}/${rel}" "${worktree}")")
        fi
    done
    # 全树递归的安装包副本（.git 剪枝，避免无谓遍历）。
    while IFS= read -r -d '' installer; do
        INSTALLER_FILES+=("${installer}")
    done < <(find "${worktree}" -name .git -prune -o -type f \( -name 'EXV-*-windows-x64-setup.exe' -o -name 'EXV*.dmg' -o -name '安装EXV.dmg' \) -print0)
done

for rel in "${CODEX_RESIDUE_DIRS[@]}"; do
    if [ -e "${REPO_ROOT}/${rel}" ]; then
        ARTIFACT_DIRS+=("$(assert_artifact_path "${REPO_ROOT}/${rel}" "${REPO_ROOT}")")
    fi
done

valid_installer() { # 仅检查明确属于 EXV 的安装包；不以文件名代替镜像有效性
    local signature
    if [ ! -f "$1" ] || [ -L "$1" ]; then return 1; fi
    case "$(basename -- "$1")" in
        EXV*.dmg|安装EXV.dmg)
            command -v hdiutil >/dev/null 2>&1 || return 1
            hdiutil verify "$1" >/dev/null 2>&1
            return $?
            ;;
    esac
    signature="$(head -c 2 -- "$1" 2>/dev/null || true)"
    [ "${signature}" = "MZ" ]
}

sha256_of() { shasum -a 256 -- "$1" | awk '{print $1}'; }

shared_residue_files() { # 共享根内安装包及其暂存/回滚残片
    if [ ! -d "${SHARED_ARTIFACT_ROOT}" ]; then return 0; fi
    find "${SHARED_ARTIFACT_ROOT}" -maxdepth 1 -type f \( \
        -name 'EXV-*-windows-x64-setup.exe' \
        -o -name 'EXV*.dmg' \
        -o -name '安装EXV.dmg' \
        -o -name 'EXV-*-windows-x64-setup.exe.exvp' \
        -o -name '.EXV-*-windows-x64-setup.staging.exe' \
        -o -name '.EXV-*-windows-x64-setup.staging.exe.exvp' \
        -o -name '.EXV-*-windows-x64-setup.previous.exe' \
        -o -name '.EXV-*-windows-x64-setup.previous.exe.exvp' \) -print
}

RETAINED_DEST=""

if [ "${KEEP_LATEST_INSTALLER}" -eq 1 ]; then
    # 最新优先（mtime 秒级相同则取字典序更小路径），与 ps1 的排序口径一致。
    best=""; best_mtime=-1
    candidates=()
    for installer in ${INSTALLER_FILES[@]+"${INSTALLER_FILES[@]}"}; do
        candidates+=("${installer}")
    done
    while IFS= read -r residue; do
        if [ -n "${residue}" ]; then candidates+=("${residue}"); fi
    done < <(shared_residue_files)
    for installer in ${candidates[@]+"${candidates[@]}"}; do
        valid_installer "${installer}" || continue
        mtime="$(stat -f %m -- "${installer}")"
        if [ -z "${best}" ] || [ "${mtime}" -gt "${best_mtime}" ] \
            || { [ "${mtime}" -eq "${best_mtime}" ] && [ "${installer}" \< "${best}" ]; }; then
            best="${installer}"
            best_mtime="${mtime}"
        fi
    done
    if [ -n "${best}" ]; then
        RETAINED_DEST="${SHARED_ARTIFACT_ROOT}/$(basename -- "${best}")"
        if [ "${WHATIF}" -eq 1 ]; then
            log "[WHATIF] 将保留最新安装包：${best} → ${RETAINED_DEST}"
        else
            if [ ! -d "${SHARED_ARTIFACT_ROOT}" ]; then mkdir -p -- "${SHARED_ARTIFACT_ROOT}"; fi
            if [ "$(canon "${best}")" != "$(canon "${RETAINED_DEST}")" ]; then
                cp -p -- "${best}" "${RETAINED_DEST}"
                [ "$(sha256_of "${best}")" = "$(sha256_of "${RETAINED_DEST}")" ] \
                    || fail "共享根安装包 sha256 不一致：${RETAINED_DEST}"
            fi
            valid_installer "${RETAINED_DEST}" || fail "共享根安装包无效：${RETAINED_DEST}"
            log "已保留最新安装包：${best} → ${RETAINED_DEST}"
        fi
    else
        log "没有可保留的有效安装包"
    fi
fi

DIRS_REMOVED=0
INSTALLERS_REMOVED=0

remove_dir() {
    if [ "${WHATIF}" -eq 1 ]; then
        log "[WHATIF] 将删除构建产物：$1"
    else
        log "删除构建产物：$1"
        rm -rf -- "$1"
    fi
    DIRS_REMOVED=$((DIRS_REMOVED + 1))
}

remove_installer() {
    if [ "${WHATIF}" -eq 1 ]; then
        log "[WHATIF] 将删除安装包副本：$1"
    else
        log "删除安装包副本：$1"
        rm -f -- "$1"
    fi
    INSTALLERS_REMOVED=$((INSTALLERS_REMOVED + 1))
}

for dir in ${ARTIFACT_DIRS[@]+"${ARTIFACT_DIRS[@]}"}; do
    if [ -e "${dir}" ]; then remove_dir "${dir}"; fi
done

for installer in ${INSTALLER_FILES[@]+"${INSTALLER_FILES[@]}"}; do
    if [ ! -f "${installer}" ]; then continue; fi
    if [ -n "${RETAINED_DEST}" ] && [ "$(canon "${installer}")" = "$(canon "${RETAINED_DEST}")" ]; then
        continue
    fi
    remove_installer "${installer}"
done

while IFS= read -r residue; do
    if [ -z "${residue}" ]; then continue; fi
    if [ -n "${RETAINED_DEST}" ] && [ "$(canon "${residue}")" = "$(canon "${RETAINED_DEST}")" ]; then
        continue
    fi
    if [ -f "${residue}" ]; then remove_installer "${residue}"; fi
done < <(shared_residue_files)

mode="APPLY"
if [ "${WHATIF}" -eq 1 ]; then mode="WHATIF"; fi
log "清理完成：mode=${mode} worktrees=${#WORKTREES[@]} directories=${DIRS_REMOVED} installers=${INSTALLERS_REMOVED} shared=${SHARED_ARTIFACT_ROOT}"
