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

    /// Single-root layout under `~/.local/share/xenolith` on every platform. We
    /// use the XDG data path verbatim (NOT the platform convention dir) on all
    /// OSes: macOS `~/Library/Application Support/…` contains a SPACE, and GNU
    /// make cannot handle a space in `STAPPLER_ROOT` / `include` paths — a project
    /// build aborts with "No such file or directory". `~/.local/share/xenolith`
    /// has no space, so it works for make on macOS too.
    pub fn system() -> Result<Self, DirsError> {
        let base = directories::BaseDirs::new().ok_or(DirsError::NoPlatformDirs)?;
        Ok(Self::from_home(
            &base.home_dir().join(".local/share/xenolith"),
        ))
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

    /// Shared, engine-independent toolchain store — the REAL host/target files,
    /// downloaded once and reused across engine versions. Each engine's own
    /// `toolchains/` dir just symlinks into here, so updating the engine never
    /// touches (or re-downloads) the toolchains.
    pub fn toolchains_store_dir(&self) -> PathBuf {
        self.data.join("toolchains")
    }

    /// Parent of all installed engine versions.
    pub fn engines_dir(&self) -> PathBuf {
        self.data.join("engines")
    }

    /// A specific engine version's root — this IS `STAPPLER_ROOT` (the build
    /// system and modules) for that version. Multiple versions coexist so a
    /// project can pick which one it builds against.
    pub fn engine_dir(&self, reference: &str) -> PathBuf {
        self.engines_dir().join(reference)
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
        assert_eq!(l.toolchains_store_dir(), Path::new("/x/data/toolchains"));
        assert_eq!(l.engines_dir(), Path::new("/x/data/engines"));
        assert_eq!(l.engine_dir("master"), Path::new("/x/data/engines/master"));
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
