//! Engine provisioning shared by the front-ends: download the engine bundle,
//! link the toolchain store into it, and record the installed version.
//!
//! Component (host/target) installation lives in [`crate::install`]; this module
//! adds the engine-bundle half so a caller can bring a machine from zero to a
//! buildable SDK.
//!
//! A local checkout can replace the baked bundle via `--engine` /
//! `$XENOLITH_ENGINE` / `settings.engine_path` — see [`resolve_engine_root`].

use std::path::{Path, PathBuf};

use crate::dirs::Layout;
use crate::engine_source::{EngineBundle, EngineInfo};
use crate::install;
use crate::settings::Settings;

/// Git ref of the engine bundle we ship (a moving `master` snapshot).
pub const ENGINE_REF: &str = "master";

/// Environment variable that overrides the engine root (`STAPPLER_ROOT`).
pub const ENGINE_ENV: &str = "XENOLITH_ENGINE";

/// `<config>/engine.json` — records the installed engine/runtime version.
pub fn engine_info_path(layout: &Layout) -> PathBuf {
    layout.config.join("engine.json")
}

/// The recorded engine version, if one is installed.
pub fn read_engine_info(layout: &Layout) -> Option<EngineInfo> {
    let bytes = std::fs::read(engine_info_path(layout)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether a path looks like a usable engine tree (`make/universal.mk` present,
/// no whitespace — GNU make cannot handle spaces in `STAPPLER_ROOT`).
pub fn validate_engine_root(path: &Path) -> Result<(), String> {
    let display = path.to_string_lossy();
    if display.contains(char::is_whitespace) {
        return Err("engine path must not contain spaces (GNU make breaks on them)".into());
    }
    if !path.join("make/universal.mk").is_file() {
        return Err(format!(
            "not a valid engine root (missing make/universal.mk): {}",
            path.display()
        ));
    }
    Ok(())
}

/// True when `root` is outside the installer's `data/engines/` tree (a live
/// checkout pointed at via `--engine` / env / settings).
pub fn is_external_engine(layout: &Layout, root: &Path) -> bool {
    let engines = layout.engines_dir();
    match (std::fs::canonicalize(root), std::fs::canonicalize(&engines)) {
        (Ok(r), Ok(e)) => !r.starts_with(&e),
        _ => {
            // Fall back to lexical compare when canonicalize fails (missing path).
            !root.starts_with(&engines)
        }
    }
}

/// Resolve `STAPPLER_ROOT` with precedence (highest first):
/// 1. `explicit` (CLI `--engine` / GUI caller)
/// 2. `$XENOLITH_ENGINE`
/// 3. `settings.engine_path`
/// 4. baked bundle at `data/engines/master`
///
/// The returned path is validated ([`validate_engine_root`]).
pub fn resolve_engine_root(layout: &Layout, explicit: Option<&Path>) -> Result<PathBuf, String> {
    let from_env = std::env::var_os(ENGINE_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let from_settings = Settings::load(layout)
        .engine_path
        .filter(|p| !p.as_os_str().is_empty());

    let candidate = explicit
        .map(|p| p.to_path_buf())
        .or(from_env)
        .or(from_settings)
        .unwrap_or_else(|| layout.engine_dir(ENGINE_REF));

    validate_engine_root(&candidate)?;
    Ok(candidate)
}

/// Resolve the engine root for a registered project.
///
/// Precedence: `$XENOLITH_ENGINE` / `settings.engine_path` (global override for
/// engine iteration) → absolute `project_engine` path → `data/engines/<ref>`.
pub fn resolve_project_engine_root(
    layout: &Layout,
    project_engine: &str,
) -> Result<PathBuf, String> {
    let from_env = std::env::var_os(ENGINE_ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from);
    let from_settings = Settings::load(layout)
        .engine_path
        .filter(|p| !p.as_os_str().is_empty());
    if let Some(p) = from_env.or(from_settings) {
        validate_engine_root(&p)?;
        return Ok(p);
    }
    let p = Path::new(project_engine);
    if p.is_absolute() {
        validate_engine_root(p)?;
        return Ok(p.to_path_buf());
    }
    let root = layout.engine_dir(project_engine);
    validate_engine_root(&root)?;
    Ok(root)
}

/// Persist a local engine path into settings (or clear it with `None`).
pub fn set_engine_path_override(layout: &Layout, path: Option<&Path>) -> Result<(), String> {
    if let Some(p) = path {
        validate_engine_root(p)?;
    }
    let mut s = Settings::load(layout);
    s.engine_path = path.map(|p| p.to_path_buf());
    s.save(layout)
}

/// Whether a usable engine tree is already in place (baked bundle **or** a
/// configured external override).
pub fn engine_installed(layout: &Layout) -> bool {
    if resolve_engine_root(layout, None).is_ok() {
        return true;
    }
    read_engine_info(layout).is_some() && layout.engine_dir(ENGINE_REF).join("make").is_dir()
}

/// Ensure the engine is available and return a version record.
///
/// When an external override is active (`--engine` / env / settings), the baked
/// bundle is **not** downloaded — toolchains are linked into the local tree and
/// a synthetic [`EngineInfo`] is returned.
///
/// Unless `force`, an already-present baked engine is returned without
/// re-downloading. `on_progress(bytes, total)` reports cumulative download
/// bytes and, when the server sent a Content-Length, the total size.
pub fn ensure_engine(
    layout: &Layout,
    force: bool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<EngineInfo, String> {
    ensure_engine_with_override(layout, None, force, on_progress)
}

/// Like [`ensure_engine`], but accepts an explicit path override (CLI `--engine`).
pub fn ensure_engine_with_override(
    layout: &Layout,
    explicit: Option<&Path>,
    force: bool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<EngineInfo, String> {
    // Explicit `--engine` must win (and fail loudly if invalid) — never fall
    // through to downloading the baked bundle.
    if let Some(p) = explicit {
        let root = {
            validate_engine_root(p)?;
            p.to_path_buf()
        };
        return use_external_engine(layout, &root, /*persist=*/ true);
    }

    // Env / settings override: skip the baked download when a local tree is set.
    if let Ok(root) = resolve_engine_root(layout, None) {
        if is_external_engine(layout, &root) {
            return use_external_engine(layout, &root, /*persist=*/ false);
        }
    }

    if !force && engine_installed_bundled(layout) {
        if let Some(info) = read_engine_info(layout) {
            return Ok(info);
        }
    }
    let info = EngineBundle::new(ENGINE_REF)
        .install(layout, on_progress)
        .map_err(|e| e.to_string())?;
    // A fresh engine ships an empty `toolchains/`; link the already-installed
    // store toolchains into it so its build can find them.
    install::link_toolchains_into_engine(layout, ENGINE_REF).map_err(|e| e.to_string())?;

    let path = engine_info_path(layout);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&info).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(info)
}

fn use_external_engine(layout: &Layout, root: &Path, persist: bool) -> Result<EngineInfo, String> {
    install::link_toolchains_into_engine_path(layout, root).map_err(|e| e.to_string())?;
    if persist {
        set_engine_path_override(layout, Some(root))?;
    }
    Ok(EngineInfo {
        reference: format!("local:{}", root.display()),
        sha256: String::new(),
    })
}

fn engine_installed_bundled(layout: &Layout) -> bool {
    read_engine_info(layout).is_some() && layout.engine_dir(ENGINE_REF).join("make").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn fake_engine(dir: &Path) {
        std::fs::create_dir_all(dir.join("make")).unwrap();
        std::fs::write(dir.join("make/universal.mk"), "# stub\n").unwrap();
    }

    #[test]
    fn validate_rejects_spaces() {
        let err = validate_engine_root(Path::new("/opt/my engine")).unwrap_err();
        assert!(err.contains("spaces"));
    }

    #[test]
    fn validate_rejects_missing_mk() {
        let dir = tempdir().unwrap();
        let err = validate_engine_root(dir.path()).unwrap_err();
        assert!(err.contains("universal.mk"));
    }

    #[test]
    fn resolve_prefers_explicit_over_settings() {
        let home = tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let bundled = layout.engine_dir(ENGINE_REF);
        fake_engine(&bundled);

        let local = home.path().join("checkout");
        fake_engine(&local);
        set_engine_path_override(&layout, Some(&local)).unwrap();

        let other = home.path().join("other");
        fake_engine(&other);

        let resolved = resolve_engine_root(&layout, Some(&other)).unwrap();
        assert_eq!(resolved, other);
    }

    #[test]
    fn resolve_falls_back_to_settings_then_bundle() {
        let home = tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let bundled = layout.engine_dir(ENGINE_REF);
        fake_engine(&bundled);

        // No override → bundled master.
        assert_eq!(resolve_engine_root(&layout, None).unwrap(), bundled);

        let local = home.path().join("checkout");
        fake_engine(&local);
        set_engine_path_override(&layout, Some(&local)).unwrap();
        assert_eq!(resolve_engine_root(&layout, None).unwrap(), local);
    }

    #[test]
    fn is_external_detects_outside_engines_dir() {
        let home = tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let bundled = layout.engine_dir(ENGINE_REF);
        fake_engine(&bundled);
        let local = home.path().join("checkout");
        fake_engine(&local);

        assert!(!is_external_engine(&layout, &bundled));
        assert!(is_external_engine(&layout, &local));
    }

    #[test]
    fn ensure_with_override_skips_download_and_persists() {
        let home = tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        // No bundled engine — only a local checkout.
        let local = home.path().join("checkout");
        fake_engine(&local);

        let info =
            ensure_engine_with_override(&layout, Some(&local), false, &mut |_, _| {}).unwrap();
        assert!(info.reference.starts_with("local:"));
        assert!(!layout.engine_dir(ENGINE_REF).exists());
        assert_eq!(
            Settings::load(&layout).engine_path.as_deref(),
            Some(local.as_path())
        );
    }
}
