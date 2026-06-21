# Runtime Assets

`runtime/` is a local, untracked staging directory for optional native runtime
assets. It is ignored by Git and must not contain files that are required to
build the repository.

Production packaging stages native binaries from the CMake build output:

- `exv` / `exv.exe`
- `exv-helper` / `exv-helper.exe`
- required MinGW runtime DLLs on Windows

Optional production runtime assets can be supplied with `EXV_RUNTIME_DIR` or
placed in an ignored local `runtime/<platform>/` directory. The current
allowlist contains only:

- `wintun.dll` for Windows native tunnel support

Packaging scripts copy allowlisted assets by exact name. They must not copy
runtime directories wholesale, and root-level build outputs such as `*.a`,
`*.lib`, `*.dll`, `*.dylib`, or `*.so` must stay out of the repository root.

Windows release packaging uses the same allowlist through
`scripts/package_ui_shell.py`. The release script packages the already-built
`build\windows\webview\package\EXV` directory into a portable zip and an NSIS
installer; it does not directly copy runtime directories. The installer does
not bundle Microsoft Edge WebView2 Evergreen Runtime. `exv-ui.exe` keeps
responsibility for detecting a missing WebView2 Evergreen Runtime and running
the controlled bootstrap flow.
