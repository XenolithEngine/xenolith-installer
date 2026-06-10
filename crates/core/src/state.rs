//! The installed-state registry (`installed.json`).
//!
//! Records what has actually been laid down on disk so the app can, on launch,
//! validate the install and compute updates against the remote manifest. The
//! file lives in the config dir (see [`crate::dirs::Layout::installed_manifest`]).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::manifest::{Component, Kind};

/// On-disk schema version, so future format changes can be migrated.
pub const SCHEMA_VERSION: u32 = 1;

/// A component recorded as installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledComponent {
    pub id: String,
    pub triple: String,
    pub variant: Option<String>,
    pub kind: Kind,
    /// Release this component came from.
    pub release: String,
    /// SHA-256 of the archive, recorded for `verify`/`repair`.
    pub sha256: Option<String>,
    /// RFC 3339 timestamp; passed in by the caller (core takes no clock).
    pub installed_at: String,
    /// Directory the archive was unpacked into.
    pub path: PathBuf,
}

impl InstalledComponent {
    /// Build an install record from a catalogue [`Component`].
    pub fn from_component(
        c: &Component,
        release: impl Into<String>,
        installed_at: impl Into<String>,
        path: PathBuf,
        sha256: Option<String>,
    ) -> Self {
        InstalledComponent {
            id: c.id.clone(),
            triple: c.triple.clone(),
            variant: c.variant.clone(),
            kind: c.kind,
            release: release.into(),
            sha256,
            installed_at: installed_at.into(),
            path,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledState {
    pub schema: u32,
    pub components: Vec<InstalledComponent>,
}

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("reading state file: {0}")]
    Io(#[from] std::io::Error),
    #[error("parsing state file: {0}")]
    Parse(#[from] serde_json::Error),
}

impl Default for InstalledState {
    fn default() -> Self {
        InstalledState {
            schema: SCHEMA_VERSION,
            components: Vec::new(),
        }
    }
}

impl InstalledState {
    /// Load the registry, returning an empty one if the file does not exist.
    pub fn load(path: &Path) -> Result<Self, StateError> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// Persist the registry, creating parent directories as needed.
    pub fn save(&self, path: &Path) -> Result<(), StateError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Insert or replace by `(id, kind)`. A triple can exist as both a host and
    /// a target (e.g. `aarch64-apple-macosx`), so id alone is NOT a unique key —
    /// keying by id only would let one overwrite the other in the registry.
    pub fn upsert(&mut self, component: InstalledComponent) {
        if let Some(slot) = self
            .components
            .iter_mut()
            .find(|c| c.id == component.id && c.kind == component.kind)
        {
            *slot = component;
        } else {
            self.components.push(component);
        }
    }

    /// Remove by `(id, kind)`; returns whether anything was removed.
    pub fn remove(&mut self, id: &str, kind: Kind) -> bool {
        let before = self.components.len();
        self.components.retain(|c| !(c.id == id && c.kind == kind));
        before != self.components.len()
    }

    /// Look up an installed component by `(id, kind)`.
    pub fn get(&self, id: &str, kind: Kind) -> Option<&InstalledComponent> {
        self.components
            .iter()
            .find(|c| c.id == id && c.kind == kind)
    }

    /// Return components whose on-disk path fails `exists` — i.e. the registry
    /// claims them installed but they are missing/corrupt. The existence check
    /// is injected so this stays a pure unit (real callers pass `Path::exists`).
    pub fn invalid<F>(&self, exists: F) -> Vec<&InstalledComponent>
    where
        F: Fn(&Path) -> bool,
    {
        self.components
            .iter()
            .filter(|c| !exists(&c.path))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn comp(id: &str) -> InstalledComponent {
        InstalledComponent {
            id: id.to_string(),
            triple: id.to_string(),
            variant: None,
            kind: Kind::Target,
            release: "rel".into(),
            sha256: Some("deadbeef".into()),
            installed_at: "2026-06-09T00:00:00Z".into(),
            path: PathBuf::from(format!("/sdk/{id}")),
        }
    }

    #[test]
    fn default_is_empty_with_current_schema() {
        let s = InstalledState::default();
        assert_eq!(s.schema, SCHEMA_VERSION);
        assert!(s.components.is_empty());
    }

    #[test]
    fn upsert_adds_then_replaces_same_id() {
        let mut s = InstalledState::default();
        s.upsert(comp("a"));
        s.upsert(comp("b"));
        assert_eq!(s.components.len(), 2);
        let mut updated = comp("a");
        updated.installed_at = "2026-12-31T00:00:00Z".into();
        s.upsert(updated);
        assert_eq!(s.components.len(), 2); // replaced, not appended
        assert_eq!(
            s.get("a", Kind::Target).unwrap().installed_at,
            "2026-12-31T00:00:00Z"
        );
    }

    #[test]
    fn same_id_different_kind_coexist() {
        // aarch64-apple-macosx as host AND target must both be tracked.
        let mut s = InstalledState::default();
        let mut host = comp("aarch64-apple-macosx");
        host.kind = Kind::Host;
        let mut target = comp("aarch64-apple-macosx");
        target.kind = Kind::Target;
        s.upsert(host);
        s.upsert(target);
        assert_eq!(s.components.len(), 2);
        assert!(s.get("aarch64-apple-macosx", Kind::Host).is_some());
        assert!(s.get("aarch64-apple-macosx", Kind::Target).is_some());
        assert!(s.remove("aarch64-apple-macosx", Kind::Host));
        assert!(s.get("aarch64-apple-macosx", Kind::Host).is_none());
        assert!(s.get("aarch64-apple-macosx", Kind::Target).is_some());
    }

    #[test]
    fn remove_reports_whether_it_removed() {
        let mut s = InstalledState::default();
        s.upsert(comp("a"));
        assert!(s.remove("a", Kind::Target));
        assert!(!s.remove("a", Kind::Target));
        assert!(s.components.is_empty());
    }

    #[test]
    fn invalid_flags_missing_paths_via_injected_check() {
        let mut s = InstalledState::default();
        s.upsert(comp("present"));
        s.upsert(comp("gone"));
        let invalid = s.invalid(|p| p != Path::new("/sdk/gone"));
        assert_eq!(invalid.len(), 1);
        assert_eq!(invalid[0].id, "gone");
    }

    #[test]
    fn load_missing_file_yields_empty_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.json");
        let s = InstalledState::load(&path).unwrap();
        assert_eq!(s, InstalledState::default());
    }

    #[test]
    fn save_then_load_round_trips_and_creates_parents() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/config/installed.json");
        let mut s = InstalledState::default();
        s.upsert(comp("x86_64-unknown-linux-gnu"));
        s.save(&path).unwrap();
        assert!(path.exists());
        let back = InstalledState::load(&path).unwrap();
        assert_eq!(s, back);
    }
}
