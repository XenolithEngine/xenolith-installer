# Changelog

All notable changes to the Xenolith Installer are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/). This project is pre-1.0
(alpha), so layouts and interfaces may still change between releases.

## [0.1.6] — 2026-07-30

### Added
- **Local engine path override** — use a live xenolith-engine checkout as
  `STAPPLER_ROOT` instead of the baked snapshot. Precedence: CLI `--engine` →
  `$XENOLITH_ENGINE` → Settings → Engine path → bundled `data/engines/master`.
  Toolchains are symlinked into the external tree; invalid paths (missing
  `make/universal.mk`, or spaces) are rejected.
- **Release builds** — GUI Debug/Release toggle on the Projects tab; CLI
  `build … --release` passes `RELEASE=1` and runs the binary from
  `stappler-build/<target>/release/`.
- **`.vscode/tasks.json`** on scaffold, plus separate debug/release launch and
  Makefile configurations (`preLaunchTask` builds the matching mode).

### Changed
- CLI catalogue flag renamed **`--sdk-release`** (alias `--catalog-release`);
  the old `--release` name now belongs to `build --release`.
- Settings (`language`, `jobs`, `enginePath`) live in shared core
  `settings.json`, used by both CLI and GUI.

### Documentation
- README documents `--engine`, `$XENOLITH_ENGINE`, `build --release`, and the
  `--sdk-release` rename.

## [0.1.5] — 2026-07-22

### Added
- **Linux ARM64 CLI builds** — `xenolith-installer-cli-aarch64-unknown-linux-musl.tar.gz`
  is now published, so ARM servers and containers no longer have to build from source.

### Fixed
- **`detect` recognises every SDK host.** The offline host list was stale (only
  4 triples), so on Linux ARM64 and RISC-V it wrongly reported "no SDK host
  available" even though the toolchains exist. It now covers the full `sdk-v0beta1`
  host set and detects the machine's libc (gnu vs musl) so it picks a host that
  actually runs on the box.
- **`install <triple>` no longer errors when a triple is both a host and a target.**
  With no `--host`/`--target` flag it now installs both in one go (a triple you both
  build on and ship to); a flag still narrows it to exactly one.

### Changed
- **Linux CLI binaries are now statically linked (musl).** The previous
  `x86_64-unknown-linux-gnu` build linked `libc`/`libm`/`libgcc_s` dynamically and
  required GLIBC ≥ 2.34, so it failed to start on Alpine, Ubuntu 20.04, Debian 11,
  RHEL 8 and most CI containers. The musl builds carry no runtime dependencies and
  run on any distro. `install.sh` now selects them, and also resolves `aarch64`.
  The `…-x86_64-unknown-linux-gnu.tar.gz` asset is still published for one release
  cycle so existing installs can `self-update`.

## [0.1.4] — 2026-07-17

### Added
- **Headless CLI parity with the GUI** — the CLI can now take a machine from zero
  to a running app with no display:
  - `install` with **no argument** provisions the whole SDK for the current host:
    the engine bundle, the native host toolchain, and the native target plus its
    `+sprt` variant (the CLI equivalent of the GUI's *Install everything*).
  - `install engine` downloads **just the engine bundle** (no toolchains);
    `install <triple>` installs a single toolchain component.
  - `new <name> [--path <dir>]` scaffolds a project — `Makefile`, `src/`,
    `.clang-format` and `.vscode/` (clangd + lldb-dap), identical to the GUI.
  - `build <path> [--target <triple>] [--run]` builds a project with the SDK
    toolchain and optionally runs the result.
- **One-line CLI installer** (`install.sh`): `curl -fsSL …/install.sh | sh` picks
  the right binary for the platform, clears the macOS quarantine flag, and puts it
  on `PATH`.

### Fixed
- **macOS: binaries crashed with `dyld: Library not loaded … liblzma.5.dylib`**
  ([#1]). The CLI/GUI dynamically linked Homebrew's liblzma from the CI build
  machine, which users don't have. liblzma is now **statically linked**
  (`LZMA_API_STATIC`), so the binaries are self-contained — no Homebrew needed.
- **CLI was pinned to a stale release.** `--release`/`--base` defaulted to a
  hardcoded `sdk-v0alpha0` instead of discovering the latest release like the GUI,
  so `list` compared against an old catalogue and could show a nonsensical
  "update available: sdk-v0alpha0" for a component already on the newer release.
  The CLI now resolves the latest release on the server automatically.
- **`list` showed bare, empty group headers** when the catalogue fetch came back
  empty (most often plain-FTP being blocked/mangled by the user's network). It now
  reports that clearly instead of looking like "nothing to install".
- README CLI examples referenced `xenolith-installer` instead of the real
  `xenolith-installer-cli` binary name.

### Documentation
- Added a **macOS CLI first-launch** note: a downloaded, unsigned CLI binary is
  quarantined and killed on launch (`zsh: killed`). Clear it once with
  `xattr -d com.apple.quarantine ./xenolith-installer-cli`.

### Known limitations
- Builds are still **unsigned** (Apple notarization / Windows signing is a
  follow-up; see `docs/RELEASING.md`).
- Package downloads use the current FTP source; an HTTPS catalogue is planned.

## [0.1.3] and earlier
See the [GitHub releases] for prior history.

[0.1.6]: https://github.com/XenolithEngine/xenolith-installer/releases/tag/v0.1.6
[0.1.5]: https://github.com/XenolithEngine/xenolith-installer/releases/tag/v0.1.5
[0.1.4]: https://github.com/XenolithEngine/xenolith-installer/releases/tag/v0.1.4
[GitHub releases]: https://github.com/XenolithEngine/xenolith-installer/releases
[#1]: https://github.com/XenolithEngine/xenolith-installer/issues/1
