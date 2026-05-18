# Build Resources

This directory contains assets used by electron-builder during packaging.

## Required Files

| File | Purpose |
|------|---------|
| `installer.nsh` | NSIS custom installer script — registers/unregisters the `exv-helper` Windows service |
| `icon.ico` | Application icon (256x256, Windows .ico format) |

## Adding the Application Icon

electron-builder looks for `icon.ico` in the `buildResources` directory for the
Windows build. If this file is missing, electron-builder falls back to its
default Electron icon.

To provide a custom icon:

1. Create or obtain a 256x256 PNG of the ECNU VPN logo
2. Convert to `.ico` format (must contain 16x16, 32x32, 48x48, and 256x256 sizes)
3. Place the file as `icon.ico` in this directory

Tools for icon creation:
- **ImageMagick**: `convert logo.png -define icon:auto-resize=256,128,64,48,32,16 icon.ico`
- **png2ico** (npm): `npx png2ico icon.ico logo-256.png logo-48.png logo-32.png logo-16.png`
- Online converters: search "png to ico"

## Notes

- The `icon.ico` file is also embedded in the portable `.exe` as the window
  and taskbar icon.
- macOS builds use `icon.icns` (not included here — place in the same directory
  or configure `mac.icon` in package.json).
- `installer.nsh` is referenced by the `nsis.include` field in `webui/package.json`.
  Do NOT rename it without updating that reference.
