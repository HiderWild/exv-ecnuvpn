# Config Platform Return and C++20 Module Pilot Design

Date: 2026-06-15

## Boundary Model

`src/core/config` is the config domain. It owns the `Config` model, load/save
logic, field updates, route settings stored in configuration, credential store
state exposed through config APIs, and config RPC contract integration.

`src/platform` is the platform integration domain. It owns OS-specific config
defaults and system path decisions, including default user agent differences,
DTLS defaults, default log file paths, and any future platform-specific config
bootstrap values.

The dependency direction is:

- config may depend on `platform/common/config_defaults.hpp`
- platform config defaults must not depend on config runtime or RPC dispatch
- platform implementation files stay beside the rest of the OS platform code

## File Layout

Common config platform abstractions live in `src/platform/common`:

- `config_defaults.hpp`

OS implementations live beside the rest of the platform implementation:

- `src/platform/win32/config_defaults.cpp`
- `src/platform/darwin/config_defaults.cpp`
- `src/platform/linux/config_defaults.cpp`

`src/core/config/platform` must not be recreated.

## C++20 Module Pilot

The config named module is intentionally narrow:

- module name: `exv.config.contract`
- module interface: `src/core/config/modules/config_contract.cppm`
- exports only read-only helpers for config canonical actions and legacy alias
  canonicalization
- uses a simple `const char*` facade to avoid exporting standard library types
  through the current MinGW/GCC modules path
- does not export JSON serialization, persisted config structures, RPC
  dispatcher code, credential storage, or runtime policy

The smoke test imports the module and compares action and alias behavior with
`src/contracts/generated/system_contract.hpp`. This keeps the module pilot tied
to the generated contract without migrating the whole config subsystem into
modules.

## Non-Goals

- No config RPC contract changes.
- No persisted config JSON shape changes.
- No credential storage behavior changes.
- No route, helper, VPN runtime, or app API behavior changes.
- No namespace cleanup beyond what is required for includes and CMake.
- No production mock/stub additions.

## Acceptance Criteria

- No production or test include uses `core/config/platform/...`.
- `CMakeLists.txt` contains no `src/core/config/platform` sources.
- `src/core/config/platform` does not exist.
- `contract_manifest_test` enforces the config platform boundary.
- `config_module_smoke_test` imports `exv.config.contract`.
- Focused config/platform tests continue to pass.
