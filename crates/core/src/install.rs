//! Install orchestration: fetch → verify → extract → place atomically.
//!
//! Ties together [`crate::transport`], [`crate::verify`], [`crate::extract`]
//! and the on-disk [`crate::dirs::Layout`]. The pipeline fails closed: a bad
//! signature, a size mismatch or an extract error aborts before anything lands
//! in the final location. Placement is atomic — extract into a staging dir that
//! is a sibling of the target (same filesystem, so `rename` is atomic), then
//! swap it in.

use std::path::PathBuf;

use crate::dirs::Layout;
use crate::extract::{self, ExtractError};
use crate::hash::sha256_hex;
use crate::manifest::{Component, Kind, Manifest};
use crate::state::InstalledComponent;
use crate::transport::{fetch_with_retries, Transport, TransportError};
use crate::verify::{Verifier, VerifyError};

/// On-disk location of an installed toolchain in the shared, engine-independent
/// store: `<data>/toolchains/<hosts|targets>/<triple>`. Engines symlink to these.
pub fn component_dir(layout: &Layout, kind: Kind, id: &str) -> PathBuf {
    layout.toolchains_store_dir().join(kind.dir()).join(id)
}

/// Relative path from directory `from_dir` to `to` (both absolute). Used so the
/// engine→store toolchain symlinks survive the whole data root being moved (e.g.
/// `~/.xenolith` → `~/.local/share/xenolith`): an absolute target dangles, a
/// relative one (`../../../../toolchains/…`) does not. Falls back to `to` if the
/// paths share no common root (different Windows drives).
fn relative_link(from_dir: &std::path::Path, to: &std::path::Path) -> std::path::PathBuf {
    let from: Vec<_> = from_dir.components().collect();
    let to_c: Vec<_> = to.components().collect();
    let common = from
        .iter()
        .zip(to_c.iter())
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return to.to_path_buf();
    }
    let mut rel = PathBuf::new();
    for _ in common..from.len() {
        rel.push("..");
    }
    for c in &to_c[common..] {
        rel.push(c.as_os_str());
    }
    if rel.as_os_str().is_empty() {
        rel.push(".");
    }
    rel
}

/// Create a directory link `dst` -> `src`: a symlink on Unix; on Windows a
/// symlink if allowed, otherwise a copy (junctions would avoid the copy but need
/// an extra crate — a TODO). The symlink target is RELATIVE so it survives the
/// data root moving; the copy fallback uses the absolute `src`.
fn link_dir(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    let rel = dst
        .parent()
        .map(|parent| relative_link(parent, src))
        .unwrap_or_else(|| src.to_path_buf());
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&rel, dst)
    }
    #[cfg(windows)]
    {
        std::os::windows::fs::symlink_dir(&rel, dst).or_else(|_| copy_dir_all(src, dst))
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = rel;
        copy_dir_all(src, dst)
    }
}

#[cfg(any(windows, not(any(unix, windows))))]
fn copy_dir_all(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_all(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), to)?;
        }
    }
    Ok(())
}

/// Remove whatever is at `path` (a symlink or a real dir) so it can be relinked.
fn clear_link(path: &std::path::Path) -> std::io::Result<()> {
    if path.is_symlink() {
        std::fs::remove_file(path).or_else(|_| std::fs::remove_dir_all(path))
    } else if path.exists() {
        std::fs::remove_dir_all(path)
    } else {
        Ok(())
    }
}

/// Symlink every toolchain from the shared store into one engine's `toolchains/`
/// dir (refreshing existing links), so that engine's build can find them.
pub fn link_toolchains_into_engine(layout: &Layout, engine_reference: &str) -> std::io::Result<()> {
    let engine_tc = layout.engine_dir(engine_reference).join("toolchains");
    for kind in [Kind::Host, Kind::Target] {
        let store_kind = layout.toolchains_store_dir().join(kind.dir());
        if !store_kind.is_dir() {
            continue;
        }
        let link_kind = engine_tc.join(kind.dir());
        std::fs::create_dir_all(&link_kind)?;
        for entry in std::fs::read_dir(&store_kind)?.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            let link = link_kind.join(entry.file_name());
            clear_link(&link)?;
            link_dir(&entry.path(), &link)?;
        }
    }
    Ok(())
}

/// Refresh toolchain links in every installed engine — call after a toolchain is
/// added or removed so all engine versions see the change.
pub fn relink_all_engines(layout: &Layout) -> std::io::Result<()> {
    let engines = layout.engines_dir();
    if !engines.is_dir() {
        return Ok(());
    }
    for entry in std::fs::read_dir(&engines)?.flatten() {
        if entry.path().is_dir() {
            if let Some(name) = entry.file_name().to_str() {
                link_toolchains_into_engine(layout, name)?;
            }
        }
    }
    Ok(())
}

/// Remove an installed component's files. Idempotent — a missing directory is
/// not an error (the caller still drops the registry entry).
pub fn uninstall(layout: &Layout, kind: Kind, id: &str) -> Result<(), std::io::Error> {
    let dir = component_dir(layout, kind, id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
}

/// The SDK archives wrap everything in a single top-level `<triple>/` dir; if
/// `dir`'s only entry is exactly that wrapper (its name matches one of `names` —
/// the component id or triple), return it so we can promote its contents and
/// avoid a doubly-nested `toolchains/<kind>/<triple>/<triple>`. A lone dir with
/// any other name (e.g. an archive whose sole top entry is `bin/`) is NOT a
/// wrapper and is left in place.
fn wrapper_dir(dir: &std::path::Path, names: &[&str]) -> Option<PathBuf> {
    let mut entries = std::fs::read_dir(dir).ok()?.flatten();
    let first = entries.next()?.path();
    if entries.next().is_none() && first.is_dir() {
        let name = first.file_name()?.to_str()?;
        if names.contains(&name) {
            return Some(first);
        }
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Downloading,
    Verifying,
    Extracting,
    Placing,
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("component not found in manifest: {0}")]
    NotFound(String),
    #[error("download size mismatch for {id}: expected {expected}, got {actual}")]
    SizeMismatch {
        id: String,
        expected: u64,
        actual: u64,
    },
    #[error(transparent)]
    Transport(#[from] TransportError),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error(transparent)]
    Extract(#[from] ExtractError),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
}

/// Wires the moving parts together for a single release. `remote_base` is the
/// directory holding `hosts/` and `targets/` (e.g. `releases/sdk-v0alpha0`).
pub struct Installer<'a> {
    pub transport: &'a dyn Transport,
    pub verifier: &'a dyn Verifier,
    pub layout: &'a Layout,
    pub remote_base: String,
    pub release: String,
}

impl<'a> Installer<'a> {
    fn remote_archive(&self, c: &Component) -> String {
        format!("{}/{}/{}", self.remote_base, c.kind.dir(), c.archive)
    }

    fn remote_sig(&self, c: &Component) -> String {
        format!("{}/{}/{}", self.remote_base, c.kind.dir(), c.sig)
    }

    /// Final on-disk location in the shared store: `<data>/toolchains/<kind>/<triple>`.
    pub fn install_dir(&self, c: &Component) -> PathBuf {
        component_dir(self.layout, c.kind, &c.id)
    }

    /// Run the full pipeline for `component_id`. `now_rfc3339` is supplied by the
    /// caller (core keeps no clock). `progress(phase, bytes)` reports coarse
    /// status; `bytes` is cumulative download bytes during [`Phase::Downloading`].
    pub fn install(
        &self,
        manifest: &Manifest,
        component_id: &str,
        now_rfc3339: &str,
        progress: &mut dyn FnMut(Phase, u64),
    ) -> Result<InstalledComponent, InstallError> {
        let component = manifest
            .find(component_id)
            .ok_or_else(|| InstallError::NotFound(component_id.to_string()))?;
        self.install_component(component, now_rfc3339, progress)
    }

    /// Install a specific, already-resolved component. Use this when an id alone
    /// is ambiguous — the same triple can exist as both a host and a target
    /// (e.g. `aarch64-apple-macosx`), so the caller picks the exact one.
    pub fn install_component(
        &self,
        component: &Component,
        now_rfc3339: &str,
        progress: &mut dyn FnMut(Phase, u64),
    ) -> Result<InstalledComponent, InstallError> {
        // 1. Download the archive and its detached signature, retrying past the
        //    server's intermittent passive-data stalls.
        progress(Phase::Downloading, 0);
        let archive = fetch_with_retries(
            self.transport,
            &self.remote_archive(component),
            4,
            &mut |n| progress(Phase::Downloading, n),
        )?;

        if archive.len() as u64 != component.size {
            return Err(InstallError::SizeMismatch {
                id: component.id.clone(),
                expected: component.size,
                actual: archive.len() as u64,
            });
        }

        let sig = fetch_with_retries(self.transport, &self.remote_sig(component), 4, &mut |_| {})?;

        // 2. Verify the signature BEFORE touching the filesystem (fail closed).
        progress(Phase::Verifying, 0);
        self.verifier.verify(&archive, &sig)?;
        let sha256 = sha256_hex(&archive);

        // 3. Extract into a staging dir that is a sibling of the final dir, so
        //    the rename in step 4 is an atomic same-filesystem move.
        progress(Phase::Extracting, 0);
        let final_dir = self.install_dir(component);
        let parent = final_dir
            .parent()
            .expect("install_dir always has a parent")
            .to_path_buf();
        std::fs::create_dir_all(&parent)?;
        let staging = parent.join(format!(".staging-{}", component.id));
        if staging.exists() {
            std::fs::remove_dir_all(&staging)?;
        }
        extract::extract_tar_xz_bytes(&archive, &staging)?;
        // Promote the archive's single top-level `<triple>/` wrapper so the
        // install is `toolchains/<kind>/<triple>`, not `…/<triple>/<triple>`.
        let placed = wrapper_dir(&staging, &[&component.id, &component.triple])
            .unwrap_or_else(|| staging.clone());

        // 4. Swap into place atomically (replacing any previous install).
        progress(Phase::Placing, 0);
        if final_dir.exists() {
            std::fs::remove_dir_all(&final_dir)?;
        }
        std::fs::rename(&placed, &final_dir)?;
        // Drop the (now-empty) staging wrapper if it wasn't what we moved.
        if placed != staging && staging.exists() {
            let _ = std::fs::remove_dir_all(&staging);
        }

        Ok(InstalledComponent::from_component(
            component,
            &self.release,
            now_rfc3339,
            final_dir,
            Some(sha256),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extract::testing::make_tar_xz;
    use crate::manifest::Manifest;
    use crate::transport::testing::MockTransport;
    use crate::verify::{AcceptAll, RejectAll};

    const REMOTE_BASE: &str = "releases/sdk-v0alpha0";

    /// A manifest with one linux host whose archive/size match a fixture we
    /// serve through the mock transport.
    fn fixture() -> (Manifest, Vec<u8>, Vec<u8>) {
        let archive = make_tar_xz(&[
            ("bin/xenolith", b"ELF", true),
            ("include/xenolith.h", b"#pragma once", false),
        ]);
        let sig = b"detached-signature".to_vec();
        let hosts = format!(
            "-rw-r--r-- 1 0 0 {} Jun 08 19:39 x86_64-unknown-linux-gnu.tar.xz\n\
             -rw-r--r-- 1 0 0 {} Jun 08 19:40 x86_64-unknown-linux-gnu.tar.xz.sig",
            archive.len(),
            sig.len()
        );
        let (manifest, _) = Manifest::from_dir_listings("sdk-v0alpha0", &hosts, "");
        (manifest, archive, sig)
    }

    fn mock(archive: &[u8], sig: &[u8]) -> MockTransport {
        MockTransport::new()
            .with_file(
                "releases/sdk-v0alpha0/hosts/x86_64-unknown-linux-gnu.tar.xz",
                archive,
            )
            .with_file(
                "releases/sdk-v0alpha0/hosts/x86_64-unknown-linux-gnu.tar.xz.sig",
                sig,
            )
    }

    #[test]
    fn installs_end_to_end_and_places_files() {
        let (manifest, archive, sig) = fixture();
        let transport = mock(&archive, &sig);
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());

        let installer = Installer {
            transport: &transport,
            verifier: &AcceptAll,
            layout: &layout,
            remote_base: REMOTE_BASE.into(),
            release: "sdk-v0alpha0".into(),
        };

        let mut phases = Vec::new();
        let rec = installer
            .install(
                &manifest,
                "x86_64-unknown-linux-gnu",
                "2026-06-09T12:00:00Z",
                &mut |p, _| {
                    if !phases.contains(&p) {
                        phases.push(p)
                    }
                },
            )
            .unwrap();

        // Files landed in <engine>/toolchains/hosts/<triple>, no staging left.
        let expected = layout
            .toolchains_store_dir()
            .join("hosts/x86_64-unknown-linux-gnu");
        assert_eq!(rec.path, expected);
        assert_eq!(
            std::fs::read(expected.join("include/xenolith.h")).unwrap(),
            b"#pragma once"
        );
        assert!(!expected
            .parent()
            .unwrap()
            .join(".staging-x86_64-unknown-linux-gnu")
            .exists());
        assert!(rec.sha256.is_some());
        assert_eq!(
            phases,
            vec![
                Phase::Downloading,
                Phase::Verifying,
                Phase::Extracting,
                Phase::Placing
            ]
        );
    }

    #[test]
    fn reinstall_replaces_previous_contents() {
        let (manifest, archive, sig) = fixture();
        let transport = mock(&archive, &sig);
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let installer = Installer {
            transport: &transport,
            verifier: &AcceptAll,
            layout: &layout,
            remote_base: REMOTE_BASE.into(),
            release: "sdk-v0alpha0".into(),
        };
        let dir = installer.install_dir(manifest.find("x86_64-unknown-linux-gnu").unwrap());
        // Pre-seed a stale file that must be gone after reinstall.
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("STALE"), b"old").unwrap();

        installer
            .install(&manifest, "x86_64-unknown-linux-gnu", "t", &mut |_, _| {})
            .unwrap();
        assert!(!dir.join("STALE").exists());
        assert!(dir.join("bin/xenolith").exists());
    }

    #[test]
    fn bad_signature_aborts_before_placing_anything() {
        let (manifest, archive, sig) = fixture();
        let transport = mock(&archive, &sig);
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let installer = Installer {
            transport: &transport,
            verifier: &RejectAll,
            layout: &layout,
            remote_base: REMOTE_BASE.into(),
            release: "sdk-v0alpha0".into(),
        };
        let err = installer
            .install(&manifest, "x86_64-unknown-linux-gnu", "t", &mut |_, _| {})
            .unwrap_err();
        assert!(matches!(err, InstallError::Verify(_)));
        // Nothing was written under the sdk root.
        assert!(!installer
            .install_dir(manifest.find("x86_64-unknown-linux-gnu").unwrap())
            .exists());
    }

    #[test]
    fn size_mismatch_is_rejected() {
        let (manifest, _archive, sig) = fixture();
        // Serve a truncated archive whose length won't match the manifest size.
        let transport = mock(b"too-short", &sig);
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let installer = Installer {
            transport: &transport,
            verifier: &AcceptAll,
            layout: &layout,
            remote_base: REMOTE_BASE.into(),
            release: "sdk-v0alpha0".into(),
        };
        let err = installer
            .install(&manifest, "x86_64-unknown-linux-gnu", "t", &mut |_, _| {})
            .unwrap_err();
        assert!(matches!(err, InstallError::SizeMismatch { .. }));
    }

    #[test]
    fn toolchains_are_linked_into_the_engine_not_copied() {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        // A toolchain in the shared store…
        let store_tc = component_dir(&layout, Kind::Host, "x86_64-unknown-linux-gnu");
        std::fs::create_dir_all(&store_tc).unwrap();
        std::fs::write(store_tc.join("host.mk"), b"HOST_CC := cc").unwrap();
        // …and an installed engine.
        std::fs::create_dir_all(layout.engine_dir("master").join("make")).unwrap();

        link_toolchains_into_engine(&layout, "master").unwrap();

        let linked = layout
            .engine_dir("master")
            .join("toolchains/hosts/x86_64-unknown-linux-gnu");
        // The toolchain is reachable through the engine…
        assert_eq!(
            std::fs::read(linked.join("host.mk")).unwrap(),
            b"HOST_CC := cc"
        );
        // …via a link, not a copy — so an engine update never duplicates it.
        #[cfg(unix)]
        assert!(linked.is_symlink());
        // Re-linking is idempotent.
        link_toolchains_into_engine(&layout, "master").unwrap();
        assert!(linked.join("host.mk").exists());
    }

    #[test]
    fn uninstall_removes_the_dir_and_is_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let dir = component_dir(&layout, Kind::Target, "x86_64-unknown-linux-gnu");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), b"x").unwrap();
        assert!(dir.exists());

        uninstall(&layout, Kind::Target, "x86_64-unknown-linux-gnu").unwrap();
        assert!(!dir.exists());
        // Removing again must not error.
        uninstall(&layout, Kind::Target, "x86_64-unknown-linux-gnu").unwrap();
    }

    #[test]
    fn archive_top_level_triple_dir_is_promoted_not_double_nested() {
        // Real SDK archives wrap files in a single `<triple>/` dir; the install
        // must be toolchains/<kind>/<triple>/bin, not …/<triple>/<triple>/bin.
        let archive = make_tar_xz(&[
            ("aarch64-apple-macosx/bin/cc", b"ELF", true),
            ("aarch64-apple-macosx/host.mk", b"HOST_CC := ...", false),
        ]);
        let sig = b"sig".to_vec();
        let hosts = format!(
            "-rw-r--r-- 1 0 0 {} Jun 08 19:39 aarch64-apple-macosx.tar.xz\n\
             -rw-r--r-- 1 0 0 {} Jun 08 19:40 aarch64-apple-macosx.tar.xz.sig",
            archive.len(),
            sig.len()
        );
        let (manifest, _) = Manifest::from_dir_listings("sdk-v0alpha0", &hosts, "");
        let transport = MockTransport::new()
            .with_file(
                "releases/sdk-v0alpha0/hosts/aarch64-apple-macosx.tar.xz",
                &archive,
            )
            .with_file(
                "releases/sdk-v0alpha0/hosts/aarch64-apple-macosx.tar.xz.sig",
                &sig,
            );
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let installer = Installer {
            transport: &transport,
            verifier: &AcceptAll,
            layout: &layout,
            remote_base: REMOTE_BASE.into(),
            release: "sdk-v0alpha0".into(),
        };
        let rec = installer
            .install(&manifest, "aarch64-apple-macosx", "t", &mut |_, _| {})
            .unwrap();
        let dir = layout
            .toolchains_store_dir()
            .join("hosts/aarch64-apple-macosx");
        assert_eq!(rec.path, dir);
        // host.mk sits directly in the toolchain dir — no extra nesting.
        assert!(dir.join("host.mk").exists());
        assert!(dir.join("bin/cc").exists());
        assert!(!dir.join("aarch64-apple-macosx").exists());
    }

    #[test]
    fn unknown_component_is_not_found() {
        let (manifest, archive, sig) = fixture();
        let transport = mock(&archive, &sig);
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let installer = Installer {
            transport: &transport,
            verifier: &AcceptAll,
            layout: &layout,
            remote_base: REMOTE_BASE.into(),
            release: "sdk-v0alpha0".into(),
        };
        let err = installer
            .install(&manifest, "no-such-triple", "t", &mut |_, _| {})
            .unwrap_err();
        assert!(matches!(err, InstallError::NotFound(_)));
    }
}
