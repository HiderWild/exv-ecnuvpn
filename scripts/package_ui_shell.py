#!/usr/bin/env python3
"""Build a native WebView shell package layout without Electron payloads."""

from __future__ import annotations

import argparse
import os
import shutil
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]


def normalized_platform(value: str | None = None) -> str:
    raw = (value or sys.platform).lower()
    if raw.startswith("win") or raw == "windows":
        return "windows"
    if raw == "darwin" or raw == "mac" or raw == "macos":
        return "macos"
    if raw.startswith("linux"):
        return "linux"
    return raw


def executable_name(stem: str, platform: str) -> str:
    return f"{stem}.exe" if platform == "windows" else stem


def first_existing(paths: list[Path], name: str) -> Path:
    for path in paths:
        if path.exists():
            return path
    checked = "\n  - ".join(str(path) for path in paths)
    raise SystemExit(f"{name} not found. Checked:\n  - {checked}")


def default_cpp_build_dirs(platform: str) -> list[Path]:
    candidates: list[Path] = []
    if os.environ.get("ECNUVPN_CPP_BUILD_DIR"):
        candidates.append(Path(os.environ["ECNUVPN_CPP_BUILD_DIR"]))
    candidates.extend(
        [
            REPO_ROOT / "build" / platform / "cpp",
            REPO_ROOT / "build-windows" / "cpp",
            REPO_ROOT / "build",
        ]
    )
    return candidates


def default_renderer_candidates(platform: str) -> list[Path]:
    if os.environ.get("ECNUVPN_RENDERER_DIST_DIR"):
        return [Path(os.environ["ECNUVPN_RENDERER_DIST_DIR"])]
    return [
        REPO_ROOT / "build" / platform / "webview" / "dist",
        REPO_ROOT / "webui" / "dist",
    ]


def copy_tree_contents(source: Path, target: Path) -> None:
    target.mkdir(parents=True, exist_ok=True)
    for entry in source.iterdir():
        destination = target / entry.name
        if entry.is_dir():
            shutil.copytree(entry, destination, dirs_exist_ok=True)
        else:
            shutil.copy2(entry, destination)


def find_binary(stem: str, platform: str) -> Path:
    name = executable_name(stem, platform)
    return first_existing(
        [candidate / name for candidate in default_cpp_build_dirs(platform)],
        f"{stem} executable",
    )


def assert_no_electron_payload(package_dir: Path) -> None:
    forbidden = ["electron.exe", "Electron Framework.framework", "chromium.pak"]
    found = [name for name in forbidden if list(package_dir.rglob(name))]
    if found:
        raise SystemExit(f"Electron/Chromium payload found: {', '.join(found)}")


def write_launch_args(package_dir: Path, platform: str) -> None:
    exv_path = Path("bin") / executable_name("exv", platform)
    renderer_index = Path("webui") / "index.html"
    args = [
        "--exv",
        exv_path.as_posix(),
        "--renderer-index",
        renderer_index.as_posix(),
    ]
    (package_dir / "exv-ui.args").write_text("\n".join(args) + "\n", encoding="utf-8")


def build_package(platform: str, output_root: Path) -> Path:
    renderer_dir = first_existing(default_renderer_candidates(platform), "Renderer build directory")

    package_dir = output_root / "ECNU VPN"
    if package_dir.exists():
        shutil.rmtree(package_dir)

    bin_dir = package_dir / "bin"
    webui_dir = package_dir / "webui"
    bin_dir.mkdir(parents=True, exist_ok=True)

    ui_binary = find_binary("exv-ui", platform)
    shutil.copy2(ui_binary, package_dir / ui_binary.name)

    for stem in ("exv", "exv-helper"):
        binary = find_binary(stem, platform)
        shutil.copy2(binary, bin_dir / binary.name)

    copy_tree_contents(renderer_dir, webui_dir)
    write_launch_args(package_dir, platform)
    assert_no_electron_payload(package_dir)
    return package_dir


def parse_args() -> argparse.Namespace:
    platform = normalized_platform(os.environ.get("ECNUVPN_BUILD_PLATFORM"))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", default=platform)
    parser.add_argument(
        "--output-root",
        type=Path,
        default=REPO_ROOT / "build" / platform / "webview" / "package",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    platform = normalized_platform(args.platform)
    package_dir = build_package(platform, args.output_root)
    print(f"packaged native WebView shell: {package_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
