# Xenolith Installer

Cross-platform installer for the [Xenolith Engine](https://github.com/XenolithEngine/xenolith-engine)
SDK (and, later, Xenolith Studio). A small download that fetches host toolchains
and target sysroots from the release server, verifies them, unpacks them, and
keeps a registry of what is installed so it can validate and update itself.

Runs as a desktop GUI **and** headless on the command line — the same binary
serves servers, CI, and Linux boxes without a display.

## Architecture

A Cargo workspace whose logic lives entirely in a UI-agnostic core; the
front-ends are thin shells over it.

```
crates/
  core/   Platform detection, manifest, download, verify, extract, install state.
          No UI, no network types leak out — transport & verification are traits,
          so everything is unit-testable without a network and swappable.
  cli/    Headless front-end (clap). No GTK/webkit. Runs anywhere.
  gui/    Desktop front-end (Tauri + Svelte).            [in progress]
```

### Core modules

| module       | responsibility                                                        |
|--------------|-----------------------------------------------------------------------|
| `triple`     | Map the running platform to a server triple; detect the **native** arch under Rosetta/WOW64; pick a host fallback when none exists. |
| `dirs`       | Per-OS config/data/cache dirs (`--prefix` › `$XENOLITH_HOME` › OS default). |
| `manifest`   | Build the catalogue (server `manifest.json`, or parse the FTP listing). Unsigned artifacts are dropped, never offered. |
| `transport`  | `Transport` trait + retry that rides out flaky FTP listings; FTP impl behind the `ftp` feature. |
| `verify`     | `Verifier` trait. Detached OpenPGP signatures; **fails closed** until the public key is pinned. |
| `hash`       | SHA-256 integrity gate.                                                |
| `extract`    | `.tar.xz` extraction preserving permissions and symlinks.             |
| `install`    | Orchestrates fetch → verify → extract → **atomic** placement (staging dir + rename). |
| `state`      | `installed.json` registry: record, validate, diff.                    |
| `catalog`    | Diff remote vs installed → per-component status for the package table. |
| `i18n`       | Fluent catalogues shared by CLI and GUI; locale resolution with English fallback. |

## CLI

```sh
xenolith-installer detect                 # native host triple
xenolith-installer list                   # catalogue with install status
xenolith-installer install <triple>       # e.g. x86_64-unknown-linux-gnu
xenolith-installer verify                 # validate the install registry
xenolith-installer update                 # components with a newer release

# Global: --lang <en|ru>  --prefix <dir>  --server host:port  --release <id>
```

Installs are verified by default and **fail closed** without a pinned signing
key. `--insecure-accept-unsigned` skips verification (development only).

## Building & testing

```sh
cargo test            # unit + integration tests (no network)
cargo run -p xenolith-installer-cli -- detect
```

## Status

- Core pipeline and CLI: implemented, test-driven.
- Signature verification: trait in place, fails closed; real OpenPGP check is
  wired once the release public key is pinned.
- GUI (Tauri + Svelte) and packaged installers: in progress.

## License

MIT
