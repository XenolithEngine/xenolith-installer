//! Persisted installer settings (`<config>/settings.json`).
//!
//! Shared by the CLI and GUI: language, parallel jobs, and an optional local
//! engine path override (`engine_path` / `$XENOLITH_ENGINE` / `--engine`).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::dirs::Layout;

/// On-disk settings. Unknown fields are ignored so older/newer front-ends can
/// share the same file.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    /// Forced UI language ("en"/"ru"/"zh"); `None` = follow the system locale.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    /// Forced `make -j` job count; `None` = one per logical CPU.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jobs: Option<u32>,
    /// Absolute path to a local engine checkout used as `STAPPLER_ROOT` instead
    /// of the baked `data/engines/<ref>` bundle. Empty/`None` = use the bundle.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub engine_path: Option<PathBuf>,
}

impl Settings {
    pub fn path(layout: &Layout) -> PathBuf {
        layout.config.join("settings.json")
    }

    /// Load settings, or defaults if the file is missing / unreadable.
    pub fn load(layout: &Layout) -> Self {
        Self::load_from(&Self::path(layout))
    }

    pub fn load_from(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    /// Persist settings (creates the parent directory if needed).
    pub fn save(&self, layout: &Layout) -> Result<(), String> {
        self.save_to(&Self::path(layout))
    }

    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, bytes).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn round_trip_preserves_engine_path() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings {
            language: Some("ru".into()),
            jobs: Some(8),
            engine_path: Some(PathBuf::from("/opt/xenolith-engine")),
        };
        s.save_to(&path).unwrap();
        let loaded = Settings::load_from(&path);
        assert_eq!(loaded, s);
    }

    #[test]
    fn missing_file_is_default() {
        let dir = tempdir().unwrap();
        let loaded = Settings::load_from(&dir.path().join("nope.json"));
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn camel_case_on_disk() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("settings.json");
        Settings {
            engine_path: Some(PathBuf::from("/e")),
            ..Default::default()
        }
        .save_to(&path)
        .unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(raw.contains("enginePath"), "got: {raw}");
    }
}
