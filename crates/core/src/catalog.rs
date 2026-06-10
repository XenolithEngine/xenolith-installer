//! The view model behind the installer's package table.
//!
//! Diffs the remote [`Manifest`] against the local [`InstalledState`] to give
//! each component a [`Status`] (Not Installed / Installed / Update Available),
//! mirroring the UI table. Rows are grouped by [`Kind`] — targets are the
//! "Runtime Platforms" group, hosts the "Development Tools" group — and the
//! component matching the current machine can be promoted to the top.

use serde::{Deserialize, Serialize};

use crate::manifest::{Kind, Manifest};
use crate::state::InstalledState;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum Status {
    NotInstalled,
    Installed,
    UpdateAvailable {
        installed_release: String,
        latest_release: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CatalogRow {
    pub id: String,
    pub triple: String,
    pub variant: Option<String>,
    pub kind: Kind,
    pub size: u64,
    pub status: Status,
}

/// Build the catalogue rows for a release by diffing remote vs installed.
pub fn build_catalog(manifest: &Manifest, state: &InstalledState) -> Vec<CatalogRow> {
    manifest
        .components
        .iter()
        .map(|c| {
            let status = match state.get(&c.id, c.kind) {
                None => Status::NotInstalled,
                Some(inst) if inst.release == manifest.release => Status::Installed,
                Some(inst) => Status::UpdateAvailable {
                    installed_release: inst.release.clone(),
                    latest_release: manifest.release.clone(),
                },
            };
            CatalogRow {
                id: c.id.clone(),
                triple: c.triple.clone(),
                variant: c.variant.clone(),
                kind: c.kind,
                size: c.size,
                status,
            }
        })
        .collect()
}

/// Installed components no longer present in the current manifest — obsolete
/// versions the UI can hide behind "Hide Obsolete Versions". Returns their ids.
pub fn obsolete_ids(manifest: &Manifest, state: &InstalledState) -> Vec<String> {
    state
        .components
        .iter()
        .filter(|c| manifest.find_kind(&c.id, c.kind).is_none())
        .map(|c| c.id.clone())
        .collect()
}

/// Stable-sort rows so the component whose id equals `native` comes first, then
/// the rest in their original order. Used to float the current machine's
/// host/target to the top of its group.
pub fn promote_native(rows: &mut [CatalogRow], native: &str) {
    // `false` (id == native) sorts before `true`; sort_by_key is stable so the
    // rest keep their order.
    rows.sort_by_key(|r| r.id != native);
}

impl CatalogRow {
    pub fn is_installed(&self) -> bool {
        matches!(self.status, Status::Installed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::InstalledComponent;
    use std::path::PathBuf;

    fn manifest() -> Manifest {
        let hosts = "\
-rw-r--r-- 1 0 0 10 Jun 08 19:39 x86_64-unknown-linux-gnu.tar.xz
-rw-r--r-- 1 0 0  1 Jun 08 19:40 x86_64-unknown-linux-gnu.tar.xz.sig
-rw-r--r-- 1 0 0 20 Jun 08 19:39 aarch64-apple-macosx.tar.xz
-rw-r--r-- 1 0 0  1 Jun 08 19:40 aarch64-apple-macosx.tar.xz.sig";
        let targets = "\
-rw-r--r-- 1 0 0 30 Jun 08 19:39 aarch64-unknown-linux-gnu.tar.xz
-rw-r--r-- 1 0 0  1 Jun 08 19:40 aarch64-unknown-linux-gnu.tar.xz.sig";
        Manifest::from_dir_listings("sdk-v0alpha1", hosts, targets).0
    }

    fn installed(id: &str, release: &str) -> InstalledComponent {
        InstalledComponent {
            id: id.into(),
            triple: id.into(),
            variant: None,
            kind: Kind::Host,
            release: release.into(),
            sha256: None,
            installed_at: "t".into(),
            path: PathBuf::from("/x"),
        }
    }

    #[test]
    fn component_only_in_manifest_is_not_installed() {
        let rows = build_catalog(&manifest(), &InstalledState::default());
        assert!(rows.iter().all(|r| r.status == Status::NotInstalled));
        assert_eq!(rows.len(), 3);
    }

    #[test]
    fn matching_release_is_installed() {
        let mut state = InstalledState::default();
        state.upsert(installed("x86_64-unknown-linux-gnu", "sdk-v0alpha1"));
        let rows = build_catalog(&manifest(), &state);
        let row = rows
            .iter()
            .find(|r| r.id == "x86_64-unknown-linux-gnu")
            .unwrap();
        assert_eq!(row.status, Status::Installed);
    }

    #[test]
    fn older_release_shows_update_available() {
        let mut state = InstalledState::default();
        state.upsert(installed("x86_64-unknown-linux-gnu", "sdk-v0alpha0"));
        let rows = build_catalog(&manifest(), &state);
        let row = rows
            .iter()
            .find(|r| r.id == "x86_64-unknown-linux-gnu")
            .unwrap();
        assert_eq!(
            row.status,
            Status::UpdateAvailable {
                installed_release: "sdk-v0alpha0".into(),
                latest_release: "sdk-v0alpha1".into(),
            }
        );
    }

    #[test]
    fn installed_but_absent_from_manifest_is_obsolete() {
        let mut state = InstalledState::default();
        state.upsert(installed("riscv64-unknown-linux-gnu", "sdk-v0alpha0"));
        assert_eq!(
            obsolete_ids(&manifest(), &state),
            vec!["riscv64-unknown-linux-gnu".to_string()]
        );
    }

    #[test]
    fn native_component_floats_to_top() {
        let mut rows = build_catalog(&manifest(), &InstalledState::default());
        promote_native(&mut rows, "aarch64-apple-macosx");
        assert_eq!(rows[0].id, "aarch64-apple-macosx");
    }

    #[test]
    fn rows_carry_kind_for_grouping() {
        let rows = build_catalog(&manifest(), &InstalledState::default());
        assert_eq!(rows.iter().filter(|r| r.kind == Kind::Host).count(), 2);
        assert_eq!(rows.iter().filter(|r| r.kind == Kind::Target).count(), 1);
    }
}
