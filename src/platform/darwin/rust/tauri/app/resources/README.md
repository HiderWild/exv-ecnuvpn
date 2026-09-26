# Darwin 菜单栏图标资产

## menu-icon-template.svg（当前托盘在用：黑白 template 剪影）

2026-09-20 用户需求「菜单栏图标希望黑白两色」取代了 MAC-SHELL-17 S2 的
「保留双色、禁用模板」拍板（历史见下节）：托盘改用 macOS 标准 template 渲染
（`tray.rs` 的 `icon_as_template(true)`），形状由 alpha 表达，系统在浅色菜单栏
画黑、深色画白。

设计为盾牌+勾号简化剪影：纯黑前景、勾号镂空（alpha 0）保证 18 pt 下可读；
球面网络纹理在剪影尺度下不可辨，不保留。母版保持与双色版相同的 12:13
纵横比（viewBox 696×754），36×36 画布按比例渲染为 34×36。

修改 SVG 后，在本目录执行以下命令同步资源（需要宿主提供 librsvg 和 Pillow）：

```sh
rsvg-convert --width 36 --height 36 --keep-aspect-ratio menu-icon-template.svg -o menu-icon-template@2x.png
python3 - <<'PY'
from pathlib import Path
from PIL import Image
image = Image.open('menu-icon-template@2x.png').convert('RGBA')
assert image.size == (34, 36)
Path('menu-icon-template@2x.rgba').write_bytes(image.tobytes())
PY
```

模板规约：非透明像素必须纯黑（RGB 全 0），形状只由 alpha 表达——重新生成后
务必自检（遍历像素断言 alpha>0 处 RGB==0）。PNG、RGBA 和 SVG 都是受版本管理的
应用资源，不是安装包或临时构建目录。

## menu-icon.svg（历史双色母版，托盘不再内嵌）

`menu-icon.svg` 沿用仓库 `assets/icons/icon.svg` 的球面网络、节点、盾牌和勾号结构，
去掉阴影，以酒红 `#9B1D35` 与暖金 `#F2CE82` 表达内部纹理，透明背景。
小尺寸连线和勾号已加粗。

当前锁定的 tray-icon 0.21.3 macOS 后端按 18 pt 高度显示图标；`menu-icon.png`
为 1× 参考，`menu-icon@2x.png` 为 Retina 资源。母版纵横比 12:13，实际尺寸
34×36 像素。同步命令：

```sh
rsvg-convert --width 36 --height 36 --keep-aspect-ratio menu-icon.svg -o menu-icon@2x.png
rsvg-convert --width 18 --height 18 --keep-aspect-ratio menu-icon.svg -o menu-icon.png
python3 - <<'PY'
from pathlib import Path
from PIL import Image
image = Image.open('menu-icon@2x.png').convert('RGBA')
assert image.size == (34, 36)
Path('menu-icon@2x.rgba').write_bytes(image.tobytes())
PY
```

PNG、RGBA 和 SVG 都是受版本管理的应用资源，不是安装包或临时构建目录。
