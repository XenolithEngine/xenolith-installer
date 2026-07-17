# Changelog

All notable changes to the Xenolith Installer are documented here. The format
follows [Keep a Changelog](https://keepachangelog.com/). This project is pre-1.0
(alpha), so layouts and interfaces may still change between releases.

## [0.1.4] — unreleased

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

### Fixed
- **macOS: binaries crashed with `dyld: Library not loaded … liblzma.5.dylib`**
  ([#1]). The CLI/GUI dynamically linked Homebrew's liblzma from the CI build
  machine, which users don't have. liblzma is now **statically linked**
  (`LZMA_API_STATIC`), so the binaries are self-contained — no Homebrew needed.
- **`list` showed bare, empty group headers** when the catalogue fetch came back
  empty (most often plain-FTP being blocked/mangled by the user's network). It now
  reports that clearly instead of looking like "nothing to install".
- README CLI examples referenced `xenolith-installer` instead of the real
  `xenolith-installer-cli` binary name.

### Known limitations
- Builds are still **unsigned** (Apple notarization / Windows signing is a
  follow-up; see `docs/RELEASING.md`).
- Package downloads use the current FTP source; an HTTPS catalogue is planned.

## [0.1.3] and earlier
See the [GitHub releases] for prior history.

[0.1.4]: https://github.com/XenolithEngine/xenolith-installer/releases
[GitHub releases]: https://github.com/XenolithEngine/xenolith-installer/releases
[#1]: https://github.com/XenolithEngine/xenolith-installer/issues/1
