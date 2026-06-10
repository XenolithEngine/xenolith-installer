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

/// On-disk location of an installed component: `<sdk>/<release>/<kind>/<id>`.
pub fn component_dir(layout: &Layout, release: &str, kind: Kind, id: &str) -> PathBuf {
    layout.sdk_root().join(release).join(kind.dir()).join(id)
}

/// Remove an installed component's files. Idempotent — a missing directory is
/// not an error (the caller still drops the registry entry).
pub fn uninstall(
    layout: &Layout,
    release: &str,
    kind: Kind,
    id: &str,
) -> Result<(), std::io::Error> {
    let dir = component_dir(layout, release, kind, id);
    if dir.exists() {
        std::fs::remove_dir_all(&dir)?;
    }
    Ok(())
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

    /// Final on-disk location: `<sdk>/<release>/<kind>/<id>`.
    pub fn install_dir(&self, c: &Component) -> PathBuf {
        self.layout
            .sdk_root()
            .join(&self.release)
            .join(c.kind.dir())
            .join(&c.id)
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

        // 4. Swap staging into place atomically (replacing any previous install).
        progress(Phase::Placing, 0);
        if final_dir.exists() {
            std::fs::remove_dir_all(&final_dir)?;
        }
        std::fs::rename(&staging, &final_dir)?;

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

        // Files landed in <sdk>/<release>/hosts/<id>, no staging left behind.
        let expected = layout
            .sdk_root()
            .join("sdk-v0alpha0/hosts/x86_64-unknown-linux-gnu");
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
    fn uninstall_removes_the_dir_and_is_idempotent() {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let dir = component_dir(&layout, "rel", Kind::Target, "x86_64-unknown-linux-gnu");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("f"), b"x").unwrap();
        assert!(dir.exists());

        uninstall(&layout, "rel", Kind::Target, "x86_64-unknown-linux-gnu").unwrap();
        assert!(!dir.exists());
        // Removing again must not error.
        uninstall(&layout, "rel", Kind::Target, "x86_64-unknown-linux-gnu").unwrap();
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
