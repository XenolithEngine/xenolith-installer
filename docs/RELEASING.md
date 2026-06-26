# Releasing & self-update

The installer updates itself from **GitHub Releases**:

- **GUI** uses the Tauri updater. On launch it checks
  `https://github.com/XenolithEngine/xenolith-installer/releases/latest/download/latest.json`,
  and if a newer **signed** build exists it shows a banner → downloads, verifies
  the minisign signature, installs and relaunches.
- **CLI** ships a `xenolith-installer-cli self-update` command that downloads the
  release asset matching the host target triple and replaces its own binary.

## One-time setup (signing key)

The updater verifies every download against a **minisign** key. The public half
is committed in `crates/gui/tauri.conf.json` (`plugins.updater.pubkey`); the
private half must live only in CI secrets.

A dev keypair was generated during bootstrap. **Before the first public release,
generate your own and rotate the pubkey** (the bootstrap key has no password and
its private half was written to a scratch dir):

```sh
cargo tauri signer generate -w ~/.xenolith-updater.key   # prompts for a password
```

Then set these **repository secrets** (Settings → Secrets → Actions):

| Secret | Value |
| --- | --- |
| `TAURI_SIGNING_PRIVATE_KEY` | the **contents** of the `.key` file |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | the password you chose (empty if none) |

…and paste the matching `.key.pub` contents into
`crates/gui/tauri.conf.json` → `plugins.updater.pubkey`.

> ⚠️ If the private key or its password is lost, you can no longer sign updates
> and clients will stop updating. Keep a backup.

## Cutting a release

1. Bump `version` in the workspace `Cargo.toml` **and** `crates/gui/tauri.conf.json`
   (they must match — the updater compares against the running app version).
2. Update `RELEASE_NOTES.md`.
3. Tag and push:
   ```sh
   git tag v0.1.1 && git push origin v0.1.1
   ```
4. The **Build installers** workflow builds every platform, signs the updater
   artifacts, and creates a **draft** release with the bundles, `latest.json`,
   and the per-target CLI tarballs.
5. **Review the draft, then publish it.** The updater only sees a *published*
   (non-draft, non-prerelease) release via the `latest` URL.

## Platform notes

- Auto-updatable bundle formats: macOS `.app.tar.gz`, Windows NSIS, Linux
  **AppImage** (deb/rpm are not self-updating).
- The CLI tarballs are named `xenolith-installer-cli-<target-triple>.tar.gz` so the
  `self_update` crate can pick the right one per host.
- macOS is built universal for the GUI; the CLI is built for both
  `aarch64-apple-darwin` and `x86_64-apple-darwin`.
