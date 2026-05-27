# Build Resources

This directory contains assets used by electron-builder during packaging.

## Application Icon

`icon.svg` is the source of truth for the ECNU VPN application icon. The same
globe + shield mark is exported into:

| File | Purpose |
|------|---------|
| `icon.svg` | Source icon and scalable Linux-style asset |
| `icon.png` | 1024x1024 PNG used by the desktop window at runtime |
| `icon.ico` | Windows executable, portable app, installer, and shortcuts |
| `icon.icns` | macOS app and DMG icon |
| `icons/*.png` | Sized PNG exports for platform tooling |

The browser favicon is the same SVG copied to `webui/public/favicon.svg`.

## Notes

- Keep these files visually identical; this project intentionally uses one app
  icon everywhere.
- `installer.nsh` is referenced by `electron-builder.config.cjs`.
  Do not rename it without updating that config.
