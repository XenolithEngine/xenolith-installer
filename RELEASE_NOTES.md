## Xenolith Installer

Cross-platform installer for the **Xenolith Engine SDK** — install the toolchains, download the engine, and create / build / run graphical projects, all from one app.

### Downloads

| Platform | File |
|----------|------|
| **macOS** (Intel + Apple Silicon) | `Xenolith Installer_*.dmg` |
| **Windows** (x64) | `Xenolith Installer_*_x64-setup.exe` / `.msi` |
| **Linux** (x64) | `*.AppImage` (portable) or `*.deb` |

### First launch (the builds are unsigned for now)

- **macOS** — Gatekeeper will say *“unidentified developer”*. Right-click the app → **Open** (once), or run:
  `xattr -dr com.apple.quarantine "Xenolith Installer.app"`
- **Windows** — SmartScreen may warn. Click **More info → Run anyway**.
- **Linux** — `chmod +x *.AppImage` and run it, or install the `.deb`.

### What's inside

- Browse and install **host toolchains + target sysroots** from the release server (GPG-verified, resumable).
- One-click **engine SDK** download (set up as `STAPPLER_ROOT`); toolchains are shared across engine versions via symlinks.
- **Projects**: create a graphical (Vulkan) window project in any folder, pick the engine version and build target, **Build / Run**, and **Open in** VS Code / Cursor / Claude Code or your file manager.
- Generated `.vscode/` config (clangd + lldb-dap wired to the toolchain) and `.clang-format`.
- English / Russian UI (follows the system locale).

### Known limitations

- Builds are **unsigned** (no Apple notarization / Windows signing yet).
- Package downloads use the current FTP source — HTTPS migration is planned.
- Alpha: the toolchain/engine layout may still change between versions.
