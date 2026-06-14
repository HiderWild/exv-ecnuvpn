#!/usr/bin/env python3
"""Generate checked-in ECNU-VPN contract artifacts."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any, Iterable


REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_MANIFEST = REPO_ROOT / "contracts" / "system.contract.json"
DEFAULT_CPP = REPO_ROOT / "src" / "contracts" / "generated" / "system_contract.hpp"
DEFAULT_TS = REPO_ROOT / "webui" / "desktop" / "shared" / "generated" / "system-contract.ts"
DEFAULT_SNAPSHOT = REPO_ROOT / "contracts" / "generated" / "system_contract_snapshot.json"


class ContractError(ValueError):
    pass


def require_object(value: Any, path: str) -> dict[str, Any]:
    if not isinstance(value, dict):
        raise ContractError(f"{path} must be an object")
    return value


def require_array(value: Any, path: str) -> list[Any]:
    if not isinstance(value, list):
        raise ContractError(f"{path} must be an array")
    return value


def require_string(value: Any, path: str) -> str:
    if not isinstance(value, str) or not value:
        raise ContractError(f"{path} must be a non-empty string")
    return value


def field_names(manifest: dict[str, Any], envelope: str, side: str) -> list[str]:
    fields = manifest["envelopes"][envelope][side]["fields"]
    return [require_string(field.get("name"), f"envelopes.{envelope}.{side}.fields[].name")
            for field in fields]


def validate_fields(fields: Any, path: str) -> None:
    seen: set[str] = set()
    for index, value in enumerate(require_array(fields, path)):
        field = require_object(value, f"{path}[{index}]")
        name = require_string(field.get("name"), f"{path}[{index}].name")
        require_string(field.get("type"), f"{path}[{index}].type")
        if not isinstance(field.get("required"), bool):
            raise ContractError(f"{path}[{index}].required must be a boolean")
        if name in seen:
            raise ContractError(f"{path} contains duplicate field {name!r}")
        seen.add(name)


def validate_string_list(values: Any, path: str) -> list[str]:
    result: list[str] = []
    seen: set[str] = set()
    for index, value in enumerate(require_array(values, path)):
        item = require_string(value, f"{path}[{index}]")
        if item in seen:
            raise ContractError(f"{path} contains duplicate value {item!r}")
        seen.add(item)
        result.append(item)
    return result


def validate_error_code_entries(values: Any, path: str) -> None:
    seen_keys: set[str] = set()
    seen_codes: set[str] = set()
    for index, value in enumerate(require_array(values, path)):
        entry = require_object(value, f"{path}[{index}]")
        key = require_string(entry.get("key"), f"{path}[{index}].key")
        code = require_string(entry.get("code"), f"{path}[{index}].code")
        if key in seen_keys:
            raise ContractError(f"{path} contains duplicate key {key!r}")
        if code in seen_codes:
            raise ContractError(f"{path} contains duplicate code {code!r}")
        seen_keys.add(key)
        seen_codes.add(code)


def validate_config(config: dict[str, Any]) -> None:
    actions = require_array(config.get("actions"), "modules.config.actions")
    action_names: set[str] = set()
    for index, value in enumerate(actions):
        action = require_object(value, f"modules.config.actions[{index}]")
        name = require_string(action.get("name"), f"modules.config.actions[{index}].name")
        if name in action_names:
            raise ContractError(f"modules.config.actions contains duplicate action {name!r}")
        action_names.add(name)

    aliases = require_array(config.get("aliases"), "modules.config.aliases")
    seen_aliases: set[str] = set()
    for index, value in enumerate(aliases):
        alias = require_object(value, f"modules.config.aliases[{index}]")
        alias_name = require_string(alias.get("alias"), f"modules.config.aliases[{index}].alias")
        target = require_string(alias.get("target"), f"modules.config.aliases[{index}].target")
        if alias_name in seen_aliases:
            raise ContractError(f"modules.config.aliases contains duplicate alias {alias_name!r}")
        if target not in action_names:
            raise ContractError(f"modules.config.aliases[{index}] targets unknown action {target!r}")
        seen_aliases.add(alias_name)
    validate_string_list(config.get("errors"), "modules.config.errors")


def validate_helper(helper: dict[str, Any]) -> None:
    validate_string_list(helper.get("messages"), "modules.helper.messages")
    validate_string_list(helper.get("capabilities"), "modules.helper.capabilities")
    validate_string_list(helper.get("errors"), "modules.helper.errors")

    ops = require_array(helper.get("ops"), "modules.helper.ops")
    op_names: set[str] = set()
    op_codes: set[int] = set()
    for index, value in enumerate(ops):
        op = require_object(value, f"modules.helper.ops[{index}]")
        name = require_string(op.get("name"), f"modules.helper.ops[{index}].name")
        code = op.get("code")
        if not isinstance(code, int) or isinstance(code, bool):
            raise ContractError(f"modules.helper.ops[{index}].code must be an integer")
        if not isinstance(op.get("requires_session"), bool):
            raise ContractError(f"modules.helper.ops[{index}].requires_session must be a boolean")
        require_string(op.get("request"), f"modules.helper.ops[{index}].request")
        require_string(op.get("response"), f"modules.helper.ops[{index}].response")
        if name in op_names:
            raise ContractError(f"modules.helper.ops contains duplicate op {name!r}")
        if code in op_codes:
            raise ContractError(f"modules.helper.ops contains duplicate code {code}")
        op_names.add(name)
        op_codes.add(code)

    security = require_object(helper.get("security"), "modules.helper.security")
    if security.get("no_credentials") is not True:
        raise ContractError("modules.helper.security.no_credentials must be true")
    forbidden = validate_string_list(security.get("forbidden_fields"),
                                     "modules.helper.security.forbidden_fields")
    for required in ("password", "cookie", "token", "auth_token", "credential"):
        if required not in forbidden:
            raise ContractError(
                f"modules.helper.security.forbidden_fields must include {required!r}"
            )


def validate_manifest(manifest: dict[str, Any]) -> None:
    if manifest.get("contract_id") != "ecnu-vpn.system":
        raise ContractError("contract_id must be ecnu-vpn.system")
    require_string(manifest.get("version"), "version")

    envelopes = require_object(manifest.get("envelopes"), "envelopes")
    for envelope in ("desktop_rpc", "core_rpc"):
        envelope_obj = require_object(envelopes.get(envelope), f"envelopes.{envelope}")
        for side in ("request", "response"):
            side_obj = require_object(envelope_obj.get(side), f"envelopes.{envelope}.{side}")
            validate_fields(side_obj.get("fields"), f"envelopes.{envelope}.{side}.fields")

    desktop = require_object(require_object(manifest.get("surfaces"), "surfaces").get("desktop_rpc"),
                             "surfaces.desktop_rpc")
    validate_string_list(desktop.get("actions"), "surfaces.desktop_rpc.actions")
    validate_string_list(desktop.get("event_types"), "surfaces.desktop_rpc.event_types")
    validate_error_code_entries(desktop.get("error_codes"), "surfaces.desktop_rpc.error_codes")

    modules = require_object(manifest.get("modules"), "modules")
    validate_config(require_object(modules.get("config"), "modules.config"))
    validate_helper(require_object(modules.get("helper"), "modules.helper"))
    for name in ("vpn", "service", "routes", "runtime", "logs"):
        module = require_object(modules.get(name), f"modules.{name}")
        if module.get("shallow") is not True:
            raise ContractError(f"modules.{name}.shallow must be true")
        validate_string_list(module.get("actions"), f"modules.{name}.actions")


def load_manifest(path: Path) -> dict[str, Any]:
    try:
        with path.open("r", encoding="utf-8") as handle:
            value = json.load(handle)
    except json.JSONDecodeError as exc:
        raise ContractError(f"{path}: invalid JSON: {exc}") from exc
    manifest = require_object(value, "manifest")
    validate_manifest(manifest)
    return manifest


def config_actions(manifest: dict[str, Any]) -> list[str]:
    return [action["name"] for action in manifest["modules"]["config"]["actions"]]


def config_aliases(manifest: dict[str, Any]) -> list[tuple[str, str]]:
    return [
        (alias["alias"], alias["target"])
        for alias in manifest["modules"]["config"]["aliases"]
    ]


def helper_ops(manifest: dict[str, Any]) -> list[str]:
    return [op["name"] for op in manifest["modules"]["helper"]["ops"]]


def desktop_error_code_map(manifest: dict[str, Any]) -> dict[str, str]:
    return {
        entry["key"]: entry["code"]
        for entry in manifest["surfaces"]["desktop_rpc"]["error_codes"]
    }


def desktop_error_codes(manifest: dict[str, Any]) -> list[str]:
    return list(desktop_error_code_map(manifest).values())


def cpp_string(value: str) -> str:
    return json.dumps(value)


def cpp_array(name: str, values: Iterable[str]) -> str:
    items = list(values)
    body = ", ".join(cpp_string(value) for value in items)
    return f"inline constexpr std::array<std::string_view, {len(items)}> {name} = {{{{{body}}}}};"


def render_cpp(manifest: dict[str, Any]) -> str:
    aliases = config_aliases(manifest)
    return "\n".join([
        "// Generated from contracts/system.contract.json. Do not edit manually.",
        "#pragma once",
        "",
        "#include <array>",
        "#include <cstddef>",
        "#include <cstdint>",
        "#include <string_view>",
        "",
        "namespace exv::contracts::generated {",
        "",
        f"inline constexpr std::string_view CONTRACT_VERSION = {cpp_string(manifest['version'])};",
        "",
        cpp_array("DESKTOP_RPC_REQUEST_FIELDS", field_names(manifest, "desktop_rpc", "request")),
        cpp_array("DESKTOP_RPC_RESPONSE_FIELDS", field_names(manifest, "desktop_rpc", "response")),
        cpp_array("CORE_RPC_REQUEST_FIELDS", field_names(manifest, "core_rpc", "request")),
        cpp_array("CORE_RPC_RESPONSE_FIELDS", field_names(manifest, "core_rpc", "response")),
        "",
        cpp_array("DESKTOP_RPC_ACTIONS", manifest["surfaces"]["desktop_rpc"]["actions"]),
        cpp_array("DESKTOP_RPC_EVENT_TYPES", manifest["surfaces"]["desktop_rpc"]["event_types"]),
        cpp_array("DESKTOP_RPC_ERROR_CODES", desktop_error_codes(manifest)),
        cpp_array("CONFIG_ACTIONS", config_actions(manifest)),
        cpp_array("CONFIG_LEGACY_ALIASES", [alias for alias, _ in aliases]),
        cpp_array("HELPER_OPS", helper_ops(manifest)),
        cpp_array("HELPER_FORBIDDEN_CREDENTIAL_FIELDS",
                  manifest["modules"]["helper"]["security"]["forbidden_fields"]),
        "",
        "struct HelperOpContract {",
        "    std::string_view name;",
        "    std::uint32_t code;",
        "    bool requires_session;",
        "};",
        "",
        f"inline constexpr std::array<HelperOpContract, {len(manifest['modules']['helper']['ops'])}> HELPER_OP_CONTRACTS = {{{{",
        *[
            f"    {{{cpp_string(op['name'])}, {op['code']}, {'true' if op['requires_session'] else 'false'}}},"
            for op in manifest["modules"]["helper"]["ops"]
        ],
        "}};",
        "",
        "struct ConfigAlias {",
        "    std::string_view alias;",
        "    std::string_view target;",
        "};",
        "",
        f"inline constexpr std::array<ConfigAlias, {len(aliases)}> CONFIG_ALIASES = {{{{",
        *[f"    {{{cpp_string(alias)}, {cpp_string(target)}}}," for alias, target in aliases],
        "}};",
        "",
        "template <std::size_t N>",
        "constexpr bool contains(const std::array<std::string_view, N>& values, std::string_view value) {",
        "    for (const auto item : values) {",
        "        if (item == value) {",
        "            return true;",
        "        }",
        "    }",
        "    return false;",
        "}",
        "",
        "constexpr bool is_desktop_rpc_action(std::string_view action) {",
        "    return contains(DESKTOP_RPC_ACTIONS, action);",
        "}",
        "",
        "constexpr bool is_config_action(std::string_view action) {",
        "    return contains(CONFIG_ACTIONS, action);",
        "}",
        "",
        "constexpr bool is_config_alias(std::string_view alias) {",
        "    return contains(CONFIG_LEGACY_ALIASES, alias);",
        "}",
        "",
        "constexpr bool is_helper_op(std::string_view op) {",
        "    return contains(HELPER_OPS, op);",
        "}",
        "",
        "constexpr bool is_helper_forbidden_credential_field(std::string_view field) {",
        "    return contains(HELPER_FORBIDDEN_CREDENTIAL_FIELDS, field);",
        "}",
        "",
        "} // namespace exv::contracts::generated",
        "",
    ])


def ts_literal(value: Any) -> str:
    return json.dumps(value, indent=2, ensure_ascii=False)


def render_ts(manifest: dict[str, Any]) -> str:
    aliases = {alias: target for alias, target in config_aliases(manifest)}
    helper = manifest["modules"]["helper"]
    return "\n".join([
        "// Generated from contracts/system.contract.json. Do not edit manually.",
        "",
        f"export const CONTRACT_VERSION = {json.dumps(manifest['version'])} as const",
        "",
        f"export const DESKTOP_RPC_REQUEST_FIELDS = {ts_literal(field_names(manifest, 'desktop_rpc', 'request'))} as const",
        f"export const DESKTOP_RPC_RESPONSE_FIELDS = {ts_literal(field_names(manifest, 'desktop_rpc', 'response'))} as const",
        f"export const CORE_RPC_REQUEST_FIELDS = {ts_literal(field_names(manifest, 'core_rpc', 'request'))} as const",
        f"export const CORE_RPC_RESPONSE_FIELDS = {ts_literal(field_names(manifest, 'core_rpc', 'response'))} as const",
        "",
        f"export const DESKTOP_RPC_ACTIONS = {ts_literal(manifest['surfaces']['desktop_rpc']['actions'])} as const",
        f"export const DESKTOP_RPC_EVENT_TYPES = {ts_literal(manifest['surfaces']['desktop_rpc']['event_types'])} as const",
        f"export const DESKTOP_RPC_ERROR_CODES = {ts_literal(desktop_error_codes(manifest))} as const",
        f"export const DESKTOP_RPC_ERROR_CODE_MAP = {ts_literal(desktop_error_code_map(manifest))} as const",
        "",
        f"export const CONFIG_ACTIONS = {ts_literal(config_actions(manifest))} as const",
        f"export const CONFIG_ALIASES = {ts_literal(aliases)} as const",
        "",
        f"export const HELPER_OPS = {ts_literal(helper_ops(manifest))} as const",
        f"export const HELPER_OP_CONTRACTS = {ts_literal(helper['ops'])} as const",
        f"export const HELPER_FORBIDDEN_CREDENTIAL_FIELDS = {ts_literal(helper['security']['forbidden_fields'])} as const",
        "",
        "export type DesktopRpcAction = (typeof DESKTOP_RPC_ACTIONS)[number]",
        "export type ConfigAction = (typeof CONFIG_ACTIONS)[number]",
        "export type HelperOp = (typeof HELPER_OPS)[number]",
        "",
    ])


def render_snapshot(manifest: dict[str, Any]) -> str:
    return json.dumps(manifest, indent=2, ensure_ascii=False, sort_keys=True) + "\n"


def write_text(path: Path, content: str, check: bool) -> bool:
    existing = path.read_text(encoding="utf-8") if path.exists() else None
    if existing == content:
        return False
    if check:
        raise ContractError(f"{path} is not up to date")
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(content, encoding="utf-8", newline="\n")
    return True


def generate(manifest_path: Path, cpp_path: Path, ts_path: Path,
             snapshot_path: Path, check: bool) -> list[Path]:
    manifest = load_manifest(manifest_path)
    outputs = {
        cpp_path: render_cpp(manifest),
        ts_path: render_ts(manifest),
        snapshot_path: render_snapshot(manifest),
    }
    changed: list[Path] = []
    for path in sorted(outputs):
        if write_text(path, outputs[path], check):
            changed.append(path)
    return changed


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--cpp", type=Path, default=DEFAULT_CPP)
    parser.add_argument("--ts", type=Path, default=DEFAULT_TS)
    parser.add_argument("--snapshot", type=Path, default=DEFAULT_SNAPSHOT)
    parser.add_argument("--check", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        changed = generate(args.manifest, args.cpp, args.ts, args.snapshot, args.check)
    except ContractError as exc:
        print(f"contract generation failed: {exc}")
        return 1

    if args.check:
        print("contract generated files are up to date")
    elif changed:
        print("generated:")
        for path in changed:
            print(f"  {path}")
    else:
        print("generated files already up to date")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
