# Utils Rewrite and C++20 Module Design

Date: 2026-06-15

## Boundary Model

`src/utils` is no longer a grab bag for platform integration. Its final scope is
pure, platform-independent value utilities that do not read files, write files,
execute processes, inspect privileges, discover bundled runtimes, touch network
interfaces, or print to the console.

The corrected ownership model is:

- `src/utils` owns pure string and value helpers.
- `src/cli` owns user-facing terminal output and ANSI console setup.
- `src/platform` owns filesystem, path, process, privilege, runtime discovery,
  network-interface, and Windows conversion details.
- `src/core`, `src/helper`, and `src/vpn_engine` depend on explicit utils,
  CLI, or platform headers instead of a monolithic `utils.hpp`.

This is a rewrite, not a compatibility-preserving cleanup. The old
`src/utils.hpp`, `src/utils.cpp`, and `src/utils_*.inc.cpp` aggregation model is
removed by the end of the migration.

## Accepted Utils Inputs And Outputs

The utils module accepts only in-memory values such as `std::string_view`,
`std::string`, and simple containers. It emits pure return values and must be
deterministic for a given input.

Allowed utilities:

- whitespace trimming
- line splitting
- small text/value helpers that are independent of OS state

Rejected utilities:

- path expansion or config directory lookup
- file existence, read, write, or directory creation
- runtime owner or ownership synchronization
- process execution, shell quoting, privilege checks, or executable lookup
- native runtime discovery
- network-interface traffic counters
- Windows wide/UTF-8 conversion and Windows error formatting
- terminal printing or ANSI console mutation

## Platform Ownership

System abstractions used by upper layers move to ordinary platform headers and
sources. Platform does not need to become fully modular in this slice; it must be
clear, buildable, and explicit.

Planned platform units:

- `platform/common/file_system.hpp` for file existence, directory creation,
  file read, and file write.
- `platform/common/runtime_paths.hpp` for effective home, config/log/tunnel
  paths, runtime owner overrides, redirect files, and ownership sync.
- `platform/common/process_utils.hpp` for command execution, shell quoting,
  executable path lookup, and privilege checks.
- `platform/common/runtime_discovery.hpp` for native bundled-runtime discovery.
- `platform/common/interface_stats.hpp` for interface traffic counters with
  platform-specific implementations.
- `platform/win32/windows_strings.hpp` for Windows-only UTF-8/wide conversion
  and system error formatting.

## CLI Ownership

Terminal output is presentation behavior, not a generic utility. CLI code owns
colored messages, headers, and Windows ANSI console setup through
`src/cli/console.hpp`.

Core, helper, and platform code may call CLI output only where they are already
acting as command-line presentation code. Domain logic should return data and
errors instead of printing.

## Module Strategy

The first named module is narrow:

- module name: `exv.utils.strings`
- module interface: `src/utils/modules/strings.cppm`
- exports only pure string helpers
- avoids platform headers and OS-specific macros

Additional utility modules can be added only after their boundary is proven to
be pure. Platform modules are intentionally deferred until the upper-layer
business modules are cleaner.

## Compatibility Policy

The end state does not keep `src/utils.hpp` as a long-term public facade.
During intermediate commits, old declarations may remain only long enough to
move consumers. The final verification gate fails if production or test code
still includes `utils.hpp`.

No production mocks or stubs are introduced. Tests may use local fakes under
`tests/` when needed, but platform behavior must remain backed by real platform
implementations.

## Acceptance Criteria

- `src/utils.hpp`, `src/utils.cpp`, and `src/utils_*.inc.cpp` no longer exist.
- `src/utils` does not include `platform/`, Windows headers, POSIX headers, or
  console IO headers.
- `src/utils` exposes at least `exv.utils.strings`.
- All system abstractions previously exposed through `utils.hpp` are owned by
  `src/platform` or `src/cli`.
- `src/core`, `src/helper`, `src/platform`, and tests include explicit new
  headers instead of `utils.hpp`.
- Existing behavior is preserved for config paths, runtime discovery, command
  execution, shell quoting, file IO, interface statistics, terminal output, and
  Windows string conversion.

