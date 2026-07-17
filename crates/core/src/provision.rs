//! Engine provisioning shared by the front-ends: download the engine bundle,
//! link the toolchain store into it, and record the installed version.
//!
//! Component (host/target) installation lives in [`crate::install`]; this module
//! adds the engine-bundle half so a caller can bring a machine from zero to a
//! buildable SDK.

use std::path::PathBuf;

use crate::dirs::Layout;
use crate::engine_source::{EngineBundle, EngineInfo};
use crate::install;

/// Git ref of the engine bundle we ship (a moving `master` snapshot).
pub const ENGINE_REF: &str = "master";

/// `<config>/engine.json` — records the installed engine/runtime version.
pub fn engine_info_path(layout: &Layout) -> PathBuf {
    layout.config.join("engine.json")
}

/// The recorded engine version, if one is installed.
pub fn read_engine_info(layout: &Layout) -> Option<EngineInfo> {
    let bytes = std::fs::read(engine_info_path(layout)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether a usable engine tree is already in place.
pub fn engine_installed(layout: &Layout) -> bool {
    read_engine_info(layout).is_some() && layout.engine_dir(ENGINE_REF).join("make").is_dir()
}

/// Ensure the engine bundle is installed and return its version.
///
/// Unless `force`, an already-present engine is returned without re-downloading.
/// `on_progress(bytes, total)` reports cumulative download bytes and, when the
/// server sent a Content-Length, the total size.
pub fn ensure_engine(
    layout: &Layout,
    force: bool,
    on_progress: &mut dyn FnMut(u64, Option<u64>),
) -> Result<EngineInfo, String> {
    if !force && engine_installed(layout) {
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
