//! Per-platform default directories, with overrides.
//!
//! Precedence (highest first):
//!   1. explicit `--prefix <path>` (CLI / GUI setting)
//!   2. `XENOLITH_HOME` environment variable
//!   3. OS conventions via the `directories` crate
//!      (`ProjectDirs::from("studio", "Xenolith", "Installer")`).
//!
//! An override `home` puts everything under one root (`home/{config,data,cache}`),
//! which is what installers/CI usually want. The OS path layout is intentionally
//! split (config vs data vs cache) because that is the platform-correct thing.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    /// Config + the installed-state registry (`installed.json`).
    pub config: PathBuf,
    /// Where the SDK itself is unpacked.
    pub data: PathBuf,
    /// Partial downloads / extraction scratch space.
    pub cache: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DirsError {
    #[error("could not determine platform directories (no $HOME?)")]
    NoPlatformDirs,
}

impl Layout {
    /// Single-root layout: `home/config`, `home/data`, `home/cache`.
    pub fn from_home(home: &Path) -> Self {
        Layout {
            config: home.join("config"),
            data: home.join("data"),
            cache: home.join("cache"),
        }
    }

    /// Platform-conventional layout via the `directories` crate.
    pub fn system() -> Result<Self, DirsError> {
        let pd = directories::ProjectDirs::from("studio", "Xenolith", "Installer")
            .ok_or(DirsError::NoPlatformDirs)?;
        Ok(Layout {
            config: pd.config_dir().to_path_buf(),
            data: pd.data_dir().to_path_buf(),
            cache: pd.cache_dir().to_path_buf(),
        })
    }

    /// Resolve using the documented precedence. `prefix` is the explicit
    /// override (CLI flag / setting); `env_home` is `$XENOLITH_HOME`.
    pub fn resolve(prefix: Option<&Path>, env_home: Option<&str>) -> Result<Self, DirsError> {
        if let Some(p) = prefix {
            return Ok(Self::from_home(p));
        }
        if let Some(h) = env_home.filter(|h| !h.is_empty()) {
            return Ok(Self::from_home(Path::new(h)));
        }
        Self::system()
    }

    /// Convenience that reads `$XENOLITH_HOME` from the real environment.
    pub fn resolve_from_env(prefix: Option<&Path>) -> Result<Self, DirsError> {
        Self::resolve(prefix, std::env::var("XENOLITH_HOME").ok().as_deref())
    }

    /// The installed-state registry file.
    pub fn installed_manifest(&self) -> PathBuf {
        self.config.join("installed.json")
    }

    /// Root under which host/target archives are unpacked, one dir per release.
    pub fn sdk_root(&self) -> PathBuf {
        self.data.join("sdk")
    }

    /// Scratch directory for in-flight downloads (atomic temp → rename target).
    pub fn download_tmp(&self) -> PathBuf {
        self.cache.join("downloads")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_home_splits_into_three_subdirs() {
        let l = Layout::from_home(Path::new("/opt/xeno"));
        assert_eq!(l.config, Path::new("/opt/xeno/config"));
        assert_eq!(l.data, Path::new("/opt/xeno/data"));
        assert_eq!(l.cache, Path::new("/opt/xeno/cache"));
    }

    #[test]
    fn prefix_wins_over_env() {
        let l = Layout::resolve(Some(Path::new("/explicit")), Some("/from/env")).unwrap();
        assert_eq!(l, Layout::from_home(Path::new("/explicit")));
    }

    #[test]
    fn env_used_when_no_prefix() {
        let l = Layout::resolve(None, Some("/from/env")).unwrap();
        assert_eq!(l, Layout::from_home(Path::new("/from/env")));
    }

    #[test]
    fn empty_env_is_ignored_and_falls_through_to_system() {
        // Empty XENOLITH_HOME must not produce paths rooted at "/config".
        let l = Layout::resolve(None, Some("")).unwrap();
        assert_ne!(l.config, Path::new("config"));
        assert!(l.config.is_absolute());
    }

    #[test]
    fn derived_paths_hang_off_the_right_roots() {
        let l = Layout::from_home(Path::new("/x"));
        assert_eq!(
            l.installed_manifest(),
            Path::new("/x/config/installed.json")
        );
        assert_eq!(l.sdk_root(), Path::new("/x/data/sdk"));
        assert_eq!(l.download_tmp(), Path::new("/x/cache/downloads"));
    }

    #[test]
    fn system_layout_is_absolute() {
        // Don't assert OS-specific strings (CI runs on all three) — just sanity.
        let l = Layout::system().unwrap();
        assert!(l.config.is_absolute());
        assert!(l.data.is_absolute());
        assert!(l.cache.is_absolute());
    }
}
