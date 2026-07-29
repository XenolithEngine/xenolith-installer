## Xenolith Installer

Cross-platform installer for the **Xenolith Engine SDK** — install the toolchains, download the engine, and create / build / run graphical projects, all from one app.

### Install the CLI — one line (macOS / Linux)

Downloads the right binary for your platform, verifies its checksum, and puts it on your `PATH`:

```sh
curl -fsSL https://raw.githubusercontent.com/XenolithEngine/xenolith-installer/main/install.sh | sh
```

Then: `xenolith-installer-cli install` → `new myapp` → `build myapp --run`. (Desktop app and manual downloads are listed below.)

### What's new in 0.1.6

- **Local engine checkout** — point the installer at a live
  [xenolith-engine](https://github.com/XenolithEngine/xenolith-engine) tree instead of the
  downloaded `engine-snapshot` bundle. Use **Settings → Engine path** in the GUI, CLI
  `--engine <path>`, or `$XENOLITH_ENGINE`. Shared toolchains are still symlinked into that
  tree's `toolchains/`, so you can iterate on the engine without re-publishing a snapshot.
- **Release builds** — GUI Projects tab has a Debug / Release switch; CLI gains
  `build … --release` (`RELEASE=1`, output under `stappler-build/<target>/release/`).
- **VS Code / Cursor tasks** — new projects get `.vscode/tasks.json` plus separate debug
  and release launch/Makefile configs, so F5 builds the matching mode first.
- **`--sdk-release`** — the catalogue release flag was renamed from `--release` so it no
  longer clashes with `build --release` (alias: `--catalog-release`).

Linux static CLI / ARM64 CLI support from **0.1.5** is unchanged.

### Downloads

**Desktop app** (GUI):

| Platform | File |
|----------|------|
| **macOS** (Intel + Apple Silicon) | `Xenolith Installer_*.dmg` |
| **Windows** (x64) | `Xenolith Installer_*_x64-setup.exe` / `.msi` |
| **Linux** (x64) | `*.AppImage` (portable) or `*.deb` |

**Headless CLI** (`xenolith-installer-cli` — for servers, CI and SSH; same install/build features, no window).

Easiest — one line (macOS / Linux); it picks the right binary and puts it on your PATH:

```sh
curl -fsSL https://raw.githubusercontent.com/XenolithEngine/xenolith-installer/main/install.sh | sh
```

Or download the tarball manually:

| Platform | File |
|----------|------|
| **macOS** (Apple Silicon / Intel) | `xenolith-installer-cli-aarch64-apple-darwin.tar.gz` / `…-x86_64-apple-darwin.tar.gz` |
| **Windows** (x64) | `xenolith-installer-cli-x86_64-pc-windows-msvc.tar.gz` |
| **Linux** (x64 / ARM64) | `xenolith-installer-cli-x86_64-unknown-linux-musl.tar.gz` / `…-aarch64-unknown-linux-musl.tar.gz` |

The Linux builds are statically linked (musl), so they need no system libraries and
run on any distro, including Alpine and minimal CI containers.

```sh
tar -xzf xenolith-installer-cli-*.tar.gz          # unpack
xattr -d com.apple.quarantine xenolith-installer-cli   # macOS only: clear quarantine
chmod +x xenolith-installer-cli
sudo mv xenolith-installer-cli /usr/local/bin/    # put it on your PATH
```

(Or skip the `mv` and run it in place as `./xenolith-installer-cli …`.)

### First launch (the builds are unsigned for now)

- **macOS (app)** — Gatekeeper will say *“unidentified developer”*. Right-click the app → **Open** (once), or run:
  `xattr -dr com.apple.quarantine "Xenolith Installer.app"`
- **macOS (CLI)** — a downloaded, unsigned binary is quarantined, so it's killed on launch (`zsh: killed`). Clear the flag once:
  `xattr -d com.apple.quarantine ./xenolith-installer-cli && chmod +x ./xenolith-installer-cli`
- **Windows** — SmartScreen may warn. Click **More info → Run anyway**.
- **Linux** — `chmod +x *.AppImage` and run it, or install the `.deb`.

### What's inside

- Browse and install **host toolchains + target sysroots** from the release server (GPG-verified, resumable).
- One-click **engine SDK** download (set up as `STAPPLER_ROOT`); toolchains are shared across engine versions via symlinks — or point at a local checkout (see above).
- **Projects**: create a graphical (Vulkan) window project in any folder, pick the engine version and build target, **Build / Run** (debug or release), and **Open in** VS Code / Cursor / Claude Code or your file manager.
- Generated `.vscode/` config (clangd + lldb-dap + build tasks, wired to the toolchain) and `.clang-format`.
- English / Russian / Chinese UI (follows the system locale).

### Known limitations

- Builds are **unsigned** (no Apple notarization / Windows signing yet).
- Package downloads use the current FTP source — HTTPS migration is planned.
- Alpha: the toolchain/engine layout may still change between versions.
