#!/usr/bin/env python3
"""Build a native WebView shell package layout without Electron payloads."""

from __future__ import annotations

import argparse
import os
import shutil
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[1]
MINGW_RUNTIME_DLLS = [
    "libgcc_s_seh-1.dll",
    "libstdc++-6.dll",
    "libwinpthread-1.dll",
]
WINDOWS_OPTIONAL_RUNTIME_DLLS = ["wintun.dll"]


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


def runtime_search_dirs(platform: str) -> list[Path]:
    candidates: list[Path] = []
    if os.environ.get("ECNUVPN_RUNTIME_DIR"):
        candidates.append(Path(os.environ["ECNUVPN_RUNTIME_DIR"]))
    candidates.extend(default_cpp_build_dirs(platform))
    if platform == "windows":
        candidates.extend(
            [
                REPO_ROOT / "runtime" / "win32-x64",
                REPO_ROOT / "runtime" / "win32",
                REPO_ROOT / "runtime" / "windows",
            ]
        )
        candidates.extend(Path(entry) for entry in os.environ.get("PATH", "").split(os.pathsep) if entry)
    unique: list[Path] = []
    seen: set[Path] = set()
    for candidate in candidates:
        normalized = candidate.resolve() if candidate.exists() else candidate
        if normalized not in seen:
            unique.append(candidate)
            seen.add(normalized)
    return unique


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


def find_webview2_loader(platform: str) -> Path | None:
    if platform != "windows":
        return None
    if os.environ.get("ECNUVPN_WEBVIEW2_LOADER_DLL"):
        return Path(os.environ["ECNUVPN_WEBVIEW2_LOADER_DLL"])
    return first_existing(
        [candidate / "WebView2Loader.dll" for candidate in default_cpp_build_dirs(platform)],
        "WebView2Loader.dll",
    )


def find_runtime_asset(name: str, platform: str) -> Path | None:
    for directory in runtime_search_dirs(platform):
        candidate = directory / name
        if candidate.exists():
            return candidate
    return None


def copy_windows_runtime_assets(package_dir: Path) -> None:
    if not package_dir.exists():
        return
    bin_dir = package_dir / "bin"
    for dll in MINGW_RUNTIME_DLLS:
        source = find_runtime_asset(dll, "windows")
        if source is None:
            checked = "\n  - ".join(str(path / dll) for path in runtime_search_dirs("windows"))
            raise SystemExit(f"{dll} not found. Checked:\n  - {checked}")
        shutil.copy2(source, package_dir / dll)
        shutil.copy2(source, bin_dir / dll)

    for dll in WINDOWS_OPTIONAL_RUNTIME_DLLS:
        source = find_runtime_asset(dll, "windows")
        if source is None:
            print(f"warning: optional Windows runtime asset not found: {dll}", file=sys.stderr)
            continue
        shutil.copy2(source, package_dir / dll)
        shutil.copy2(source, bin_dir / dll)


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


def validate_launch_args_targets(package_dir: Path) -> None:
    args_path = package_dir / "exv-ui.args"
    args = [line.strip() for line in args_path.read_text(encoding="utf-8").splitlines()]
    for token in args:
        if not token or token.startswith("--"):
            continue
        target = (package_dir / token).resolve()
        package_root = package_dir.resolve()
        if package_root not in (target, *target.parents) or not target.exists():
            raise SystemExit(f"Launch args target not found: {token}")


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

    webview2_loader = find_webview2_loader(platform)
    if webview2_loader:
        shutil.copy2(webview2_loader, package_dir / "WebView2Loader.dll")

    for stem in ("exv", "exv-helper"):
        binary = find_binary(stem, platform)
        shutil.copy2(binary, bin_dir / binary.name)

    if platform == "windows":
        copy_windows_runtime_assets(package_dir)

    copy_tree_contents(renderer_dir, webui_dir)
    write_launch_args(package_dir, platform)
    validate_launch_args_targets(package_dir)
    assert_no_electron_payload(package_dir)
    return package_dir


def parse_args() -> argparse.Namespace:
    platform = normalized_platform(os.environ.get("ECNUVPN_BUILD_PLATFORM"))
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", default=platform)
    parser.add_argument("--verify-launch-targets-only", action="store_true")
    parser.add_argument("--package-dir", type=Path)
    parser.add_argument(
        "--output-root",
        type=Path,
        default=REPO_ROOT / "build" / platform / "webview" / "package",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    platform = normalized_platform(args.platform)
    if args.verify_launch_targets_only:
        if not args.package_dir:
            raise SystemExit("--package-dir is required with --verify-launch-targets-only")
        validate_launch_args_targets(args.package_dir)
        assert_no_electron_payload(args.package_dir)
        print(f"verified native WebView shell package: {args.package_dir}")
        return 0

    package_dir = build_package(platform, args.output_root)
    print(f"packaged native WebView shell: {package_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
