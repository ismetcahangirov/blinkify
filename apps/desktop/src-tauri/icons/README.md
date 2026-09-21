# Application icons — placeholders

Generated from `../app-icon.png` by `tauri icon`, and deliberately ugly: a flat
shell-background square with a cross. They exist so `tauri build` has the files
`tauri.conf.json` names, not because anyone designed them.

**[#20](https://github.com/ismetcahangirov/blinkify/issues/20) replaces the
whole set**, derived from the Blinkify logo, along with the window chrome and
the installer artwork.

The Android and iOS output `tauri icon` also produces has been deleted. Windows
first, deliberately — see
[ADR-0001](../../../../docs/decisions/ADR-0001-tauri-over-electron.md).

To regenerate after replacing `app-icon.png`:

```bash
pnpm --filter @blinkify/desktop exec tauri icon src-tauri/app-icon.png
```
