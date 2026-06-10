//! Tauri desktop shell. Thin async wrappers over `xenolith-installer-core`:
//! the heavy FTP/verify/extract work runs on a blocking thread and progress is
//! pushed to the webview as events.

use serde::Serialize;
use tauri::{AppHandle, Emitter};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use std::collections::HashMap;

use xenolith_installer_core::{
    catalog::{build_catalog, promote_native, CatalogRow},
    dirs::Layout,
    i18n::I18n,
    install::{self, Installer, Phase},
    key_source,
    manifest::{self, Kind},
    state::InstalledState,
    transport_ftp::FtpTransport,
    triple::{self, resolve_host},
    verify::PgpVerifier,
};

fn parse_kind(kind: &str) -> Result<Kind, String> {
    match kind {
        "host" => Ok(Kind::Host),
        "target" => Ok(Kind::Target),
        other => Err(format!("unknown component kind: {other}")),
    }
}

const SERVER: &str = "stappler.dev:21";
const REMOTE_BASE: &str = "/releases/sdk-v0alpha0";
const RELEASE: &str = "sdk-v0alpha0";
const LIST_ATTEMPTS: u32 = 4;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogDto {
    release: String,
    native_id: Option<String>,
    rows: Vec<CatalogRow>,
    dropped: Vec<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressDto {
    id: String,
    phase: &'static str,
    bytes: u64,
}

fn phase_str(p: Phase) -> &'static str {
    match p {
        Phase::Downloading => "downloading",
        Phase::Verifying => "verifying",
        Phase::Extracting => "extracting",
        Phase::Placing => "placing",
    }
}

fn layout() -> Result<Layout, String> {
    Layout::resolve_from_env(None).map_err(|e| e.to_string())
}

#[tauri::command]
fn detect_host() -> Result<String, String> {
    let arch = std::env::consts::ARCH;
    let os = std::env::consts::OS;
    match resolve_host(arch, os).map_err(|e| e.to_string())? {
        Some(h) if h.via_emulation => Ok(format!("{} (via {})", h.native, h.host_archive)),
        Some(h) => Ok(h.native),
        None => Ok(format!("no SDK host for {arch}-{os}")),
    }
}

#[tauri::command]
fn translate(key: String, args: Option<HashMap<String, String>>) -> String {
    let i18n = I18n::from_env();
    match args {
        Some(m) if !m.is_empty() => {
            let pairs: Vec<(&str, &str)> =
                m.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
            i18n.get_args(&key, &pairs)
        }
        _ => i18n.get(&key),
    }
}

#[tauri::command]
async fn uninstall_component(id: String, kind: String) -> Result<(), String> {
    let kind = parse_kind(&kind)?;
    tauri::async_runtime::spawn_blocking(move || uninstall_blocking(&id, kind))
        .await
        .map_err(|e| e.to_string())?
}

fn uninstall_blocking(id: &str, kind: Kind) -> Result<(), String> {
    let layout = layout()?;
    install::uninstall(&layout, RELEASE, kind, id).map_err(|e| e.to_string())?;
    let path = layout.installed_manifest();
    let mut state = InstalledState::load(&path).map_err(|e| e.to_string())?;
    state.remove(id, kind);
    state.save(&path).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn load_catalog() -> Result<CatalogDto, String> {
    tauri::async_runtime::spawn_blocking(load_catalog_blocking)
        .await
        .map_err(|e| e.to_string())?
}

fn load_catalog_blocking() -> Result<CatalogDto, String> {
    let layout = layout()?;
    let transport = FtpTransport::new(SERVER);
    let (manifest, dropped) =
        manifest::fetch_manifest(&transport, REMOTE_BASE, RELEASE, LIST_ATTEMPTS)
            .map_err(|e| e.to_string())?;
    let state = InstalledState::load(&layout.installed_manifest()).map_err(|e| e.to_string())?;
    let mut rows = build_catalog(&manifest, &state);
    let native = triple::host_triple_from(std::env::consts::ARCH, std::env::consts::OS).ok();
    if let Some(n) = &native {
        promote_native(&mut rows, n);
    }
    Ok(CatalogDto {
        release: manifest.release,
        native_id: native,
        rows,
        dropped,
    })
}

#[tauri::command]
async fn install_component(app: AppHandle, id: String, kind: String) -> Result<(), String> {
    let kind = parse_kind(&kind)?;
    tauri::async_runtime::spawn_blocking(move || install_blocking(&app, &id, kind))
        .await
        .map_err(|e| e.to_string())?
}

fn install_blocking(app: &AppHandle, id: &str, kind: Kind) -> Result<(), String> {
    let layout = layout()?;
    let transport = FtpTransport::new(SERVER);

    // Establish the trusted signing key (fetched from a keyserver, pinned).
    let asc = key_source::fetch_release_key().map_err(|e| e.to_string())?;
    let verifier = PgpVerifier::release(&asc).map_err(|e| e.to_string())?;

    let (manifest, _) = manifest::fetch_manifest(&transport, REMOTE_BASE, RELEASE, LIST_ATTEMPTS)
        .map_err(|e| e.to_string())?;

    // Resolve the exact component: a triple can exist as both host and target.
    let component = manifest
        .find_kind(id, kind)
        .ok_or_else(|| format!("component not found: {id} ({kind:?})"))?
        .clone();

    let installer = Installer {
        transport: &transport,
        verifier: &verifier,
        layout: &layout,
        remote_base: REMOTE_BASE.into(),
        release: RELEASE.into(),
    };
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();

    // Throttle download events to ~one per whole percent so the UI bar is smooth
    // without flooding the bridge (the transport reports every 64 KiB chunk).
    let size = component.size.max(1);
    let mut last_pct = u64::MAX;
    let record = installer
        .install_component(&component, &now, &mut |phase, bytes| {
            let emit = if phase == Phase::Downloading {
                let p = bytes * 100 / size;
                let changed = p != last_pct;
                last_pct = p;
                changed
            } else {
                true // verify / extract / place fire once each
            };
            if emit {
                let _ = app.emit(
                    "install://progress",
                    ProgressDto {
                        id: id.to_string(),
                        phase: phase_str(phase),
                        bytes,
                    },
                );
            }
        })
        .map_err(|e| e.to_string())?;

    let path = layout.installed_manifest();
    let mut state = InstalledState::load(&path).map_err(|e| e.to_string())?;
    state.upsert(record);
    state.save(&path).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            detect_host,
            translate,
            load_catalog,
            install_component,
            uninstall_component
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Xenolith installer");
}
