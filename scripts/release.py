#!/usr/bin/env python3
"""EXV 产品版本的读取、同步和只读检查；仅使用 Python 3.9+ 标准库。"""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

CONFIGS = tuple(f"src/platform/{host}/rust/tauri/app/tauri.conf.json" for host in ("win32", "darwin"))
CRATES = {
    "src/platform/win32/rust/crates/exv-vpn-win32-engine/Cargo.toml": "exv-engine",
    "src/platform/win32/rust/crates/exv-vpn-win32-host/Cargo.toml": "exv-core",
    "src/platform/win32/rust/tauri/app/Cargo.toml": "exv-ui",
    "src/platform/darwin/rust/crates/exv-vpn-darwin-core/Cargo.toml": "exv-vpn-darwin-core",
    "src/platform/darwin/rust/crates/exv-vpn-darwin-engine/Cargo.toml": "exv-vpn-darwin-engine",
    "src/platform/darwin/rust/crates/exv-vpn-darwin-service-agent/Cargo.toml": "exv-vpn-darwin-service-agent",
    "src/platform/darwin/rust/tauri/app/Cargo.toml": "exv-vpn-darwin-tauri",
}
LOCKS = ("Cargo.lock", "src/platform/win32/rust/Cargo.lock", "src/platform/win32/rust/tauri/Cargo.lock", "src/platform/darwin/rust/Cargo.lock")
SEMVER = re.compile(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?")


def validate_version(value):
    match = SEMVER.fullmatch(value) if isinstance(value, str) else None
    if not match or (match.group(4) and any(part.isdigit() and len(part) > 1 and part[0] == "0" for part in match.group(4).split("."))):
        raise ValueError(f"无效产品版本（需要 SemVer）：{value}")
    return value


def product_version(root):
    value = json.loads((root / "release.json").read_bytes().decode("utf-8-sig"))
    return validate_version(value["version"])


def build_flags(root):
    """沿用 Cargo 原选项优先级，以单元分隔符传递含空格的路径映射。"""
    encoded = os.environ.get("CARGO_ENCODED_RUSTFLAGS")
    flags = (encoded.split("\x1f") if encoded else []) if encoded is not None else os.environ.get("RUSTFLAGS", "").split()
    mappings = [(Path.home(), "/home/user")]
    if os.environ.get("CARGO_HOME"):
        mappings.append((Path(os.environ["CARGO_HOME"]).resolve(), "/cargo"))
    git_environment = {key: value for key, value in os.environ.items() if key not in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_COMMON_DIR")}
    common = subprocess.run(["git", "-C", str(root), "rev-parse", "--path-format=absolute", "--git-common-dir"], capture_output=True, encoding="utf-8", env=git_environment)
    if common.returncode == 0:
        common_dir = Path(common.stdout.strip()).resolve()
        if common_dir.name == ".git" and common_dir.parent != root.resolve():
            mappings.append((common_dir.parent, "/src/exv"))
    mappings.append((root.resolve(), "/src/exv"))
    flags.extend(f"--remap-path-prefix={source}={target}" for source, target in mappings)
    return "\x1f".join(flags)


def replace_json_version(text, version):
    value = json.loads(text)
    if not isinstance(value.get("version"), str):
        raise ValueError("缺少字符串 version 字段")
    # 保留文件布局；受管理配置的 version 是首个同名字段。
    updated, count = re.subn(r'("version"\s*:\s*)"[^"\r\n]*"', lambda m: m[1] + json.dumps(version), text, count=1)
    if count != 1:
        raise ValueError("缺少 version 字段")
    return updated


def replace_package_version(text, version, name):
    match = re.search(r"(?ms)^\[package\]\s*\n.*?(?=^\[|\Z)", text)
    if not match or not re.search(r'^name\s*=\s*"' + re.escape(name) + r'"\s*$', match[0], re.M):
        raise ValueError(f"缺少预期 package：{name}")
    block, count = re.subn(r'(?m)^(version\s*=\s*)"[^"\r\n]*"', lambda m: m[1] + json.dumps(version), match[0], count=1)
    if count != 1:
        raise ValueError(f"{name} 缺少 package.version")
    return text[:match.start()] + block + text[match.end():]


def replace_lock_versions(text, version):
    def update(match):
        block = match[0]
        name = re.search(r'^name\s*=\s*"([^"]+)"', block, re.M)
        if name and name[1] in CRATES.values() and not re.search(r"^source\s*=", block, re.M):
            block = re.sub(r'(?m)^(version\s*=\s*)"[^"\r\n]*"', lambda m: m[1] + json.dumps(version), block, count=1)
        return block
    return re.sub(r"(?ms)^\[\[package\]\].*?(?=^\[\[package\]\]|\Z)", update, text)


def prepared_files(root, version):
    """先读取并验证全部受管理文件，再返回修改集合；缺文件时不落盘。"""
    files = {}
    for path in ("release.json", *CONFIGS, *CRATES, *LOCKS):
        try:
            original = (root / path).read_bytes()
            text = original.decode("utf-8")
            newline = "\r\n" if "\r\n" in text else "\n"
            normalized = text.replace("\r\n", "\n")
            if path == "release.json" or path in CONFIGS:
                updated = replace_json_version(normalized, version)
            elif path in CRATES:
                updated = replace_package_version(normalized, version, CRATES[path])
            else:
                updated = replace_lock_versions(normalized, version)
            files[path] = (original, updated.replace("\n", newline).encode("utf-8"))
        except (OSError, ValueError) as error:
            raise ValueError(f"{path}: {error}") from error
    return files


def check(root, expected=None, tag=False, notes=False):
    version = product_version(root)
    errors = []
    for path, (original, updated) in prepared_files(root, version).items():
        if original != updated:
            errors.append(f"{path}: 版本与 release.json 的 {version} 不一致；运行 version set {version}")
    if expected and expected != version:
        errors.append(f"请求版本 {expected} 与 release.json 的 {version} 不一致")
    if notes:
        entries = json.loads((root / "release-notes.json").read_bytes().decode("utf-8"))
        base_version = version.split("+", 1)[0].split("-", 1)[0]
        current = [entry for entry in entries if entry.get("version") == base_version]
        if len(current) != 1:
            errors.append(f"release-notes.json: {base_version} 需要一条版本记录，实际 {len(current)} 条")
        else:
            for host in ("windows", "macos"):
                items = current[0].get(host)
                if not isinstance(items, list) or not items or not all(isinstance(item, str) and item.strip() for item in items):
                    errors.append(f"release-notes.json: {base_version} 缺少 {host} 用户更新日志")
    if tag:
        result = subprocess.run(["git", "-C", str(root), "tag", "--points-at", "HEAD"], capture_output=True, text=True, check=True)
        tags = [line for line in result.stdout.splitlines() if re.fullmatch(r"v[0-9].*", line)]
        if tags != [f"v{version}"]:
            errors.append(f"HEAD 版本标签为 {', '.join(tags) or '无'}，需要 v{version}")
    if errors:
        raise ValueError("\n".join(errors))
    return version


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("build-flags", help="输出去除宿主路径的 Cargo 编译选项")
    version_parser = commands.add_parser("version")
    actions = version_parser.add_subparsers(dest="action", required=True)
    actions.add_parser("show")
    setter = actions.add_parser("set")
    setter.add_argument("value")
    checker = actions.add_parser("check")
    checker.add_argument("--expect")
    checker.add_argument("--tag", action="store_true")
    checker.add_argument("--notes", action="store_true")
    args = parser.parse_args(argv)
    try:
        if args.command == "build-flags":
            print(build_flags(args.root))
            return 0
        if args.action == "show":
            print(product_version(args.root))
        elif args.action == "set":
            value = validate_version(args.value)
            files = prepared_files(args.root, value)
            for path, (original, updated) in files.items():
                if original != updated:
                    (args.root / path).write_bytes(updated)
                    print(path)
            print(f"产品版本：{value}")
        else:
            print(check(args.root, args.expect, args.tag, args.notes))
    except (OSError, ValueError, KeyError, subprocess.CalledProcessError) as error:
        print(str(error), file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
