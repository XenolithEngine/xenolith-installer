## Xenolith Installer

Cross-platform installer for the **Xenolith Engine SDK** — install the toolchains, download the engine, and create / build / run graphical projects, all from one app.

### Install the CLI — one line (macOS / Linux)

Downloads the right binary for your platform, verifies its checksum, and puts it on your `PATH`:

```sh
curl -fsSL https://raw.githubusercontent.com/XenolithEngine/xenolith-installer/main/install.sh | sh
```

Then: `xenolith-installer-cli install` → `new myapp` → `build myapp --run`. (Desktop app and manual downloads are listed below.)

### What's new in 0.1.5

- **The Linux CLI is now a static binary** — no system libraries at all. The previous
  build linked glibc dynamically and needed GLIBC ≥ 2.34, so it refused to start on
  Alpine, Ubuntu 20.04, Debian 11, RHEL 8 and most CI containers. The musl builds run
  anywhere, which means `curl … | sh` now works inside a bare container.
- **Linux ARM64 is now built** (`aarch64-unknown-linux-musl`). ARM servers, Raspberry-Pi-class
  boards and ARM CI runners no longer have to compile the CLI from source — `install.sh`
  picks the right binary automatically.

Everything else is unchanged from 0.1.4, which brought the full headless CLI flow
(`install` → `new` → `build … --run`), the macOS `liblzma` launch fix, and automatic
tracking of the latest SDK release.

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
- One-click **engine SDK** download (set up as `STAPPLER_ROOT`); toolchains are shared across engine versions via symlinks.
- **Projects**: create a graphical (Vulkan) window project in any folder, pick the engine version and build target, **Build / Run**, and **Open in** VS Code / Cursor / Claude Code or your file manager.
- Generated `.vscode/` config (clangd + lldb-dap wired to the toolchain) and `.clang-format`.
- English / Russian UI (follows the system locale).

### Known limitations

- Builds are **unsigned** (no Apple notarization / Windows signing yet).
- Package downloads use the current FTP source — HTTPS migration is planned.
- Alpha: the toolchain/engine layout may still change between versions.
