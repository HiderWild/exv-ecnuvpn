#!/usr/bin/env python3
"""生成 Darwin 安装介质素材；图形由矢量路径绘制，不改变应用图标。"""
from __future__ import annotations

import argparse
from pathlib import Path

REPAIR_NAME = "提示已损坏时运行.command"
# Finder 的状态栏、路径栏会占用 DMG 窗口的内容区，且用户可以在 Finder 中随时
# 切换它们。背景画布必须小于 760×600 的窗口可视区，才能保持初始页及标题栏缩放
# 往返时都无需滚动。
WINDOW = (760, 500)
ICON_SIZE = 96
LOCATIONS = {"EXV.app": (190, 140), "Applications": (570, 140),
             REPAIR_NAME: (190, 325), "安装说明.txt": (570, 325)}
GUIDE = """安装 EXV

1. 将 EXV.app 拖到右侧「应用程序」，再从「应用程序」打开。
2. 若提示「已损坏」，运行「提示已损坏时运行.command」后重试；仍失败请重新下载。
3. 若无法验证开发者，在「系统设置 → 隐私与安全性」点击「仍要打开」。
4. 适用于 M 系列 Mac，macOS 11 或更高版本。

工具仅移除隔离标记，不修复实际损坏。
"""
REPAIR = '''#!/bin/bash
# 仅用于 EXV 已出现「已损坏」提示时移除隔离标记。
set -u
APP_PATH="/Applications/EXV.app"
if [[ ! -d "${APP_PATH}" ]]; then
    echo "请先把 EXV.app 拖到「应用程序」，再运行本工具。"
    read -r -p "按回车键关闭…" _
    exit 1
fi
echo "本工具仅移除 EXV 的隔离标记，不能修复实际损坏。"
if RESULT="$(/usr/bin/xattr -dr com.apple.quarantine "${APP_PATH}" 2>&1)"; then
    echo "隔离标记处理完成，请重新打开 EXV；若仍失败，请重新下载或反馈提示。"
else
    case "${RESULT}" in
        *"Permission denied"*|*"Operation not permitted"*) ;;
        *)
            echo "处理未完成：${RESULT}"
            echo "请重新打开 EXV；若仍失败，请重新下载或反馈提示。"
            read -r -p "按回车键关闭…" _
            exit 1
            ;;
    esac
    echo "需要管理员权限，请按系统提示输入密码。"
    if /usr/bin/sudo /usr/bin/xattr -dr com.apple.quarantine "${APP_PATH}"; then
        echo "隔离标记处理完成，请重新打开 EXV；若仍失败，请重新下载或反馈提示。"
    else
        echo "处理失败，请保留上方错误并反馈。"
        read -r -p "按回车键关闭…" _
        exit 1
    fi
fi
read -r -p "按回车键关闭…" _
'''


def validate_layout():
    """将中文标签余量纳入包围盒，验证完整图标之间的可见间隔。"""
    boxes = []
    for name, (x, y) in LOCATIONS.items():
        box = (x - 150, y - ICON_SIZE / 2, x + 150, y + ICON_SIZE / 2 + 44)
        assert 0 <= box[0] < box[2] <= WINDOW[0], name
        assert 0 <= box[1] < box[3] <= WINDOW[1], name
        for other in boxes:
            assert box[2] <= other[0] or other[2] <= box[0] or box[3] <= other[1] or other[3] <= box[1], name
        boxes.append(box)
    return boxes


def generate_assets(output: Path, app_icon: Path):
    from PIL import Image, ImageDraw, ImageFont

    validate_layout()
    output.mkdir(parents=True, exist_ok=True)
    font_path = "/System/Library/Fonts/Supplemental/Arial Unicode.ttf"
    def font(size):
        return ImageFont.truetype(font_path, size)

    # 双倍采样后缩回逻辑尺寸，避免 Finder 按像素显示导致背景裁切。
    canvas = Image.new("RGB", (WINDOW[0] * 2, WINDOW[1] * 2), "#f3f7fc")
    draw = ImageDraw.Draw(canvas)
    def text(x, y, value, size, fill="#203654"):
        draw.text((x * 2, y * 2), value, font=font(size * 2), fill=fill)
    draw.rounded_rectangle((48, 28, 1472, 430), radius=40, fill="#ffffff")
    text(54, 24, "安装 EXV", 30)
    text(54, 60, "拖到「应用程序」，即可开始", 17, "#536a84")
    # 安装方向箭头是独立矢量路径。
    draw.line([(646, 280), (874, 280)], fill="#438cce", width=6)
    draw.line([(846, 258), (874, 280), (846, 302)], fill="#438cce", width=6)
    text(54, 235, "需要帮助？", 17)
    text(54, 255, "仅在出现对应提示时使用左侧工具", 13, "#536a84")
    text(54, 463, "安装完成后，可推出此磁盘", 13, "#6b7e96")
    canvas.resize(WINDOW, Image.Resampling.LANCZOS).save(output / "bg.png")

    with Image.open(app_icon) as source:
        icon = source.convert("RGBA").resize((1024, 1024), Image.Resampling.LANCZOS)
    badge = ImageDraw.Draw(icon)
    # 左下小蓝箱：三个多边形形成顶面、左面、右面及封箱带。
    badge.rounded_rectangle((12, 652, 382, 1020), radius=72, fill="#ffffff")
    badge.polygon([(50, 752), (190, 690), (338, 750), (196, 818)], fill="#71c8ff")
    badge.polygon([(50, 752), (196, 818), (196, 982), (50, 910)], fill="#298bd4")
    badge.polygon([(196, 818), (338, 750), (338, 914), (196, 982)], fill="#1267ad")
    badge.polygon([(108, 726), (252, 793), (281, 778), (137, 713)], fill="#c6eaff")
    badge.polygon([(252, 793), (281, 778), (281, 824), (252, 839)], fill="#a5dcff")
    icon.save(output / "InstallerIcon.icns", format="ICNS")
    icon.save(output / "InstallerIcon.png")
    (output / "安装说明.txt").write_text(GUIDE, encoding="utf-8")
    repair = output / REPAIR_NAME
    repair.write_text(REPAIR, encoding="utf-8")
    repair.chmod(0o755)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--app-icon", type=Path, required=True)
    args = parser.parse_args()
    generate_assets(args.output, args.app_icon)
