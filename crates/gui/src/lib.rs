//! Tauri desktop shell. Thin async wrappers over `xenolith-installer-core`:
//! the heavy FTP/verify/extract work runs on a blocking thread and progress is
//! pushed to the webview as events.

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use tauri_plugin_updater::UpdaterExt;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use std::collections::HashMap;

use xenolith_installer_core::{
    catalog::{build_catalog, promote_native, CatalogRow},
    dirs::Layout,
    engine_source::{EngineBundle, EngineInfo},
    i18n::I18n,
    install::{self, Installer, Phase},
    key_source,
    manifest::{self, Kind},
    projects::{self, Project, ProjectRegistry},
    releases,
    state::InstalledState,
    transport::Transport,
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
/// Root holding one directory per release; the newest is selected at runtime so
/// a freshly-published pre-release (e.g. beta after alpha) surfaces without a
/// code change. See [`resolve_release`].
const RELEASES_ROOT: &str = "/releases";
/// Used only if listing `/releases` fails or finds nothing recognisable.
const FALLBACK_RELEASE: &str = "sdk-v0alpha0";
const LIST_ATTEMPTS: u32 = 4;
/// Engine bundle ref to install as STAPPLER_ROOT (temporary GH-release source).
const ENGINE_REF: &str = "master";

/// Serializes the shared-state sections of concurrent installs/uninstalls: the
/// engine download (so it happens once) and the `installed.json` read-modify-write
/// plus engine relink (so parallel installs don't clobber each other's records).
/// Downloads and extraction stay parallel — they touch only their own dir.
static INSTALL_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Lock `INSTALL_LOCK`, recovering the guard if a previous holder panicked.
fn install_guard() -> std::sync::MutexGuard<'static, ()> {
    INSTALL_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Discover the newest published release by listing `/releases`, falling back to
/// [`FALLBACK_RELEASE`] if the listing fails or holds nothing recognisable.
/// Returns `(remote_base, release_name)`.
fn resolve_release(transport: &dyn Transport) -> (String, String) {
    let fallback = || {
        (
            format!("{RELEASES_ROOT}/{FALLBACK_RELEASE}"),
            FALLBACK_RELEASE.to_string(),
        )
    };
    match releases::latest_release(transport, RELEASES_ROOT, LIST_ATTEMPTS) {
        Ok(Some(r)) => {
            log::info!("selected release {}", r.name);
            (r.base(RELEASES_ROOT), r.name)
        }
        Ok(None) => {
            log::warn!("no release found under {RELEASES_ROOT}; using {FALLBACK_RELEASE}");
            fallback()
        }
        Err(e) => {
            log::warn!("release discovery failed ({e}); using {FALLBACK_RELEASE}");
            fallback()
        }
    }
}

/// PID of the in-flight build/run child (0 = none), so `cancel_build` can stop it.
static BUILD_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// `<config>/engine.json` — records the installed engine/runtime version.
fn engine_info_path(layout: &Layout) -> std::path::PathBuf {
    layout.config.join("engine.json")
}

fn read_engine_info(layout: &Layout) -> Option<EngineInfo> {
    let bytes = std::fs::read(engine_info_path(layout)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

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
    kind: &'static str,
    phase: &'static str,
    bytes: u64,
}

fn kind_str(k: Kind) -> &'static str {
    match k {
        Kind::Host => "host",
        Kind::Target => "target",
    }
}

/// The installed engine/runtime version, for display.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineDto {
    reference: String,
    short: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EngineProgressDto {
    phase: &'static str,
    bytes: u64,
    /// Server-reported total size (0 = unknown → show an indeterminate bar).
    total: u64,
}

fn engine_dto(info: &EngineInfo) -> EngineDto {
    EngineDto {
        reference: info.reference.clone(),
        short: info.short(),
    }
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
    // The data-root override lives OUTSIDE the data root (a bootstrap file in the
    // OS config dir), since it decides where the data root is.
    let prefix = data_root_override();
    Layout::resolve(
        prefix.as_deref(),
        std::env::var("XENOLITH_HOME").ok().as_deref(),
    )
    .map_err(|e| e.to_string())
}

/// Bootstrap file (in the OS config dir, independent of the data root) holding a
/// user-chosen data-root override.
fn data_root_bootstrap() -> Option<std::path::PathBuf> {
    directories::BaseDirs::new()
        .map(|b| b.config_dir().join("xenolith-installer").join("data-root"))
}

fn data_root_override() -> Option<std::path::PathBuf> {
    let p = std::fs::read_to_string(data_root_bootstrap()?).ok()?;
    let trimmed = p.trim();
    (!trimmed.is_empty()).then(|| std::path::PathBuf::from(trimmed))
}

/// Persisted UI/build settings, stored at `<config>/settings.json` (the data-root
/// override is NOT here — see `data_root_bootstrap`).
#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    /// Forced UI language ("en"/"ru"); None = follow the system locale.
    language: Option<String>,
    /// Forced `make -j` job count; None = one per logical CPU.
    jobs: Option<u32>,
}

fn settings_path(layout: &Layout) -> std::path::PathBuf {
    layout.config.join("settings.json")
}

fn load_settings() -> Settings {
    layout()
        .ok()
        .and_then(|l| std::fs::read(settings_path(&l)).ok())
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn auto_jobs() -> u32 {
    std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4)
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
fn translate(key: String, args: Option<HashMap<String, String>>, lang: Option<String>) -> String {
    let i18n = match lang.as_deref() {
        Some(l) if !l.is_empty() => I18n::new(l),
        _ => I18n::from_env(),
    };
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
    log::info!("uninstall_component id={id} kind={kind}");
    let kind = parse_kind(&kind)?;
    let id_log = id.clone();
    tauri::async_runtime::spawn_blocking(move || uninstall_blocking(&id, kind))
        .await
        .map_err(|e| e.to_string())?
        .inspect_err(|e| log::error!("uninstall {id_log} failed: {e}"))
}

fn uninstall_blocking(id: &str, kind: Kind) -> Result<(), String> {
    let layout = layout()?;
    install::uninstall(&layout, kind, id).map_err(|e| e.to_string())?;
    let path = layout.installed_manifest();
    let _g = install_guard();
    let mut state = InstalledState::load(&path).map_err(|e| e.to_string())?;
    state.remove(id, kind);
    state.save(&path).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
async fn engine_status() -> Result<Option<EngineDto>, String> {
    let layout = layout()?;
    Ok(read_engine_info(&layout).map(|i| engine_dto(&i)))
}

#[tauri::command]
async fn prepare_engine(app: AppHandle) -> Result<EngineDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let layout = layout()?;
        ensure_engine(&app, &layout, true).map(|i| engine_dto(&i))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Ensure the engine bundle (STAPPLER_ROOT) is unpacked. Returns the recorded
/// version without re-downloading if it is already present, unless `force`.
fn ensure_engine(app: &AppHandle, layout: &Layout, force: bool) -> Result<EngineInfo, String> {
    if !force {
        if let Some(info) = read_engine_info(layout) {
            if layout.engine_dir(ENGINE_REF).join("make").is_dir() {
                return Ok(info);
            }
        }
    }
    log::info!("ensure_engine: downloading engine '{ENGINE_REF}'");
    let mut last_step = u64::MAX;
    let info = EngineBundle::new(ENGINE_REF)
        .install(layout, &mut |bytes, total| {
            // Throttle to ~one event per 256 KiB downloaded.
            let step = bytes / (256 * 1024);
            if step != last_step {
                last_step = step;
                let _ = app.emit(
                    "engine://progress",
                    EngineProgressDto {
                        phase: "downloading",
                        bytes,
                        total: total.unwrap_or(0),
                    },
                );
            }
        })
        .map_err(|e| e.to_string())?;

    // A fresh engine ships an empty toolchains/ — link the already-installed
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
    let _ = app.emit(
        "engine://progress",
        EngineProgressDto {
            phase: "done",
            bytes: 0,
            total: 0,
        },
    );
    Ok(info)
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
    let (remote_base, release) = resolve_release(&transport);
    let (manifest, dropped) =
        manifest::fetch_manifest(&transport, &remote_base, &release, LIST_ATTEMPTS)
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
    log::info!("install_component id={id} kind={kind}");
    let kind = parse_kind(&kind)?;
    let id_log = id.clone();
    tauri::async_runtime::spawn_blocking(move || install_blocking(&app, &id, kind))
        .await
        .map_err(|e| e.to_string())?
        .inspect_err(|e| log::error!("install {id_log} failed: {e}"))
}

fn install_blocking(app: &AppHandle, id: &str, kind: Kind) -> Result<(), String> {
    let layout = layout()?;
    // Toolchains install into <engine>/toolchains, so the engine (STAPPLER_ROOT)
    // must exist first. Downloads it on first use; no-op afterwards. Hold the lock
    // so concurrent installs don't both download the engine.
    {
        let _g = install_guard();
        ensure_engine(app, &layout, false)?;
    }
    let transport = FtpTransport::new(SERVER);
    let (remote_base, release) = resolve_release(&transport);
    // Establish the trusted signing key (fetched from a keyserver, pinned).
    let asc = key_source::fetch_release_key().map_err(|e| e.to_string())?;
    let verifier = PgpVerifier::release(&asc).map_err(|e| e.to_string())?;
    let (manifest, _) = manifest::fetch_manifest(&transport, &remote_base, &release, LIST_ATTEMPTS)
        .map_err(|e| e.to_string())?;
    let installer = Installer {
        transport: &transport,
        verifier: &verifier,
        layout: &layout,
        remote_base,
        release,
    };
    install_one(app, &installer, &manifest, &layout, id, kind)
}

/// Download + verify + extract + place ONE component using an already-built
/// installer + manifest. Factored out so a multi-component install (the one-click
/// bootstrap) fetches the key/manifest ONCE instead of per component — otherwise
/// the slow per-component FTP round-trips leave the UI stalled between steps.
fn install_one(
    app: &AppHandle,
    installer: &Installer,
    manifest: &manifest::Manifest,
    layout: &Layout,
    id: &str,
    kind: Kind,
) -> Result<(), String> {
    // A triple can exist as both host and target — resolve the exact one.
    let component = manifest
        .find_kind(id, kind)
        .ok_or_else(|| format!("component not found: {id} ({kind:?})"))?
        .clone();
    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    // Throttle download events to ~one per whole percent so the UI bar is smooth
    // without flooding the bridge (the transport reports every 64 KiB chunk).
    let size = component.size.max(1);
    let kstr = kind_str(kind);
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
                        kind: kstr,
                        phase: phase_str(phase),
                        bytes,
                    },
                );
            }
        })
        .map_err(|e| e.to_string())?;

    // Serialize the read-modify-write of installed.json + the engine relink so
    // parallel installs (e.g. host + target of the same triple) don't clobber
    // each other's records.
    let path = layout.installed_manifest();
    let _g = install_guard();
    let mut state = InstalledState::load(&path).map_err(|e| e.to_string())?;
    state.upsert(record);
    state.save(&path).map_err(|e| e.to_string())?;
    install::relink_all_engines(layout).map_err(|e| e.to_string())?;
    Ok(())
}

/// One-click bootstrap: engine + the native host toolchain + the native targets
/// (plain and `+sprt`). Fetches the key/manifest ONCE and reuses them across all
/// components so the UI never stalls between steps on redundant FTP round-trips.
#[tauri::command]
async fn install_for_system(app: AppHandle) -> Result<(), String> {
    log::info!("install_for_system");
    tauri::async_runtime::spawn_blocking(move || {
        let host = native_host()?;
        let layout = layout()?;
        {
            let _g = install_guard();
            ensure_engine(&app, &layout, false)?;
        }
        let transport = FtpTransport::new(SERVER);
        let (remote_base, release) = resolve_release(&transport);
        let asc = key_source::fetch_release_key().map_err(|e| e.to_string())?;
        let verifier = PgpVerifier::release(&asc).map_err(|e| e.to_string())?;
        let (manifest, _) =
            manifest::fetch_manifest(&transport, &remote_base, &release, LIST_ATTEMPTS)
                .map_err(|e| e.to_string())?;
        let installer = Installer {
            transport: &transport,
            verifier: &verifier,
            layout: &layout,
            remote_base,
            release,
        };

        install_one(&app, &installer, &manifest, &layout, &host, Kind::Host)
            .inspect_err(|e| log::error!("install_for_system host failed: {e}"))?;

        // Plain native target (current default) + its self-contained `+sprt` variant.
        let mut targets = Vec::new();
        if manifest.find_kind(&host, Kind::Target).is_some() {
            targets.push(host.clone());
        }
        let sprt = format!("{host}+sprt");
        if manifest.find_kind(&sprt, Kind::Target).is_some() {
            targets.push(sprt);
        }
        for t in targets {
            install_one(&app, &installer, &manifest, &layout, &t, Kind::Target)
                .inspect_err(|e| log::error!("install_for_system target {t} failed: {e}"))?;
        }
        Ok(())
    })
    .await
    .map_err(|e| e.to_string())?
}

// ---------- storage / disk usage ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageItem {
    id: String,
    bytes: u64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StorageDto {
    engines: Vec<StorageItem>,
    hosts: Vec<StorageItem>,
    targets: Vec<StorageItem>,
    total: u64,
}

/// Recursively sum the byte size of a directory (best-effort; skips errors).
/// Does NOT follow symlinks — engines symlink the toolchain store into themselves,
/// so following them would double-count the toolchains against each engine.
fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0;
    if let Ok(rd) = std::fs::read_dir(path) {
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            } else if ft.is_dir() {
                total += dir_size(&entry.path());
            } else {
                total += entry.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    total
}

fn list_sizes(dir: &std::path::Path) -> Vec<StorageItem> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for entry in rd.flatten() {
            if entry.path().is_dir() {
                out.push(StorageItem {
                    id: entry.file_name().to_string_lossy().into_owned(),
                    bytes: dir_size(&entry.path()),
                });
            }
        }
    }
    out.sort_by(|a, b| b.bytes.cmp(&a.bytes));
    out
}

#[tauri::command]
async fn disk_usage() -> Result<StorageDto, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let layout = layout()?;
        let store = layout.toolchains_store_dir();
        let engines = list_sizes(&layout.engines_dir());
        let hosts = list_sizes(&store.join("hosts"));
        let targets = list_sizes(&store.join("targets"));
        let total = engines
            .iter()
            .chain(&hosts)
            .chain(&targets)
            .map(|i| i.bytes)
            .sum();
        Ok(StorageDto {
            engines,
            hosts,
            targets,
            total,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Delete an installed engine version directory.
#[tauri::command]
fn remove_engine(reference: String) -> Result<(), String> {
    log::info!("remove_engine {reference}");
    let layout = layout()?;
    let dir = layout.engine_dir(&reference);
    if dir.is_dir() {
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// On-disk size of a project directory (mostly its `stappler-build/` output).
#[tauri::command]
async fn project_size(path: String) -> Result<u64, String> {
    tauri::async_runtime::spawn_blocking(move || dir_size(std::path::Path::new(&path)))
        .await
        .map_err(|e| e.to_string())
}

// ---------- settings ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsDto {
    language: Option<String>,
    jobs: Option<u32>,
    auto_jobs: u32,
    data_dir: String,
    default_data_dir: String,
    data_dir_override: Option<String>,
}

fn root_of(layout: &Layout) -> String {
    layout
        .config
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_default()
}

#[tauri::command]
fn get_settings() -> Result<SettingsDto, String> {
    let s = load_settings();
    let data_dir = root_of(&layout()?);
    let default_data_dir = Layout::system()
        .ok()
        .map(|l| root_of(&l))
        .unwrap_or_default();
    Ok(SettingsDto {
        language: s.language,
        jobs: s.jobs,
        auto_jobs: auto_jobs(),
        data_dir,
        default_data_dir,
        data_dir_override: data_root_override().map(|p| p.display().to_string()),
    })
}

#[tauri::command]
fn set_settings(language: Option<String>, jobs: Option<u32>) -> Result<(), String> {
    let s = Settings {
        language: language.filter(|l| !l.is_empty()),
        jobs: jobs.filter(|n| *n >= 1),
    };
    log::info!("set_settings language={:?} jobs={:?}", s.language, s.jobs);
    let layout = layout()?;
    let path = settings_path(&layout);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&s).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())
}

/// Persist (or clear) the data-root override. Takes effect on the next launch.
#[tauri::command]
fn set_data_dir(path: Option<String>) -> Result<(), String> {
    let boot = data_root_bootstrap().ok_or("no OS config directory")?;
    if let Some(parent) = boot.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    match path.map(|p| p.trim().to_string()).filter(|p| !p.is_empty()) {
        Some(p) => {
            // GNU make splits STAPPLER_ROOT on whitespace — a space breaks builds.
            if p.contains(char::is_whitespace) {
                return Err("data directory must not contain spaces".into());
            }
            log::info!("set_data_dir -> {p}");
            std::fs::write(&boot, p).map_err(|e| e.to_string())?;
        }
        None => {
            log::info!("set_data_dir -> default (cleared)");
            let _ = std::fs::remove_file(&boot);
        }
    }
    Ok(())
}

// ---------- diagnostics / doctor ----------

fn ids_of(dir: &std::path::Path) -> String {
    let ids: Vec<String> = list_sizes(dir).into_iter().map(|i| i.id).collect();
    if ids.is_empty() {
        "—".into()
    } else {
        ids.join(", ")
    }
}

/// A plain-text diagnostics dump (system + install state + log tail) for support.
#[tauri::command]
fn diagnostics_report() -> Result<String, String> {
    use std::fmt::Write;
    let layout = layout()?;
    let store = layout.toolchains_store_dir();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "Xenolith Installer {} — {} {}",
        env!("CARGO_PKG_VERSION"),
        std::env::consts::OS,
        std::env::consts::ARCH
    );
    let _ = writeln!(s, "Data dir : {}", root_of(&layout));
    let _ = writeln!(
        s,
        "Engine   : {}",
        read_engine_info(&layout)
            .map(|e| format!("{} ({})", e.reference, e.short()))
            .unwrap_or_else(|| "not installed".into())
    );
    let _ = writeln!(
        s,
        "Host     : {}",
        native_host().unwrap_or_else(|_| "unknown".into())
    );
    let _ = writeln!(s, "Hosts    : {}", ids_of(&store.join("hosts")));
    let _ = writeln!(s, "Targets  : {}", ids_of(&store.join("targets")));
    let set = load_settings();
    let _ = writeln!(s, "Settings : lang={:?} jobs={:?}", set.language, set.jobs);
    let _ = writeln!(s, "\n--- installer.log (last 150 lines) ---");
    if let Ok(content) = std::fs::read_to_string(log_dir().join("installer.log")) {
        let lines: Vec<&str> = content.lines().collect();
        let start = lines.len().saturating_sub(150);
        s.push_str(&lines[start..].join("\n"));
    }
    Ok(s)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DoctorCheck {
    name: String,
    ok: bool,
    detail: String,
}

/// Health checks over the install — catches the things that silently break a build.
#[tauri::command]
fn run_doctor() -> Result<Vec<DoctorCheck>, String> {
    let layout = layout()?;
    let mut out = Vec::new();
    let mut check = |name: &str, ok: bool, detail: String| {
        out.push(DoctorCheck {
            name: name.into(),
            ok,
            detail,
        })
    };

    let eng = layout.engine_dir(ENGINE_REF);
    let has_make = eng.join("make").join("universal.mk").is_file();
    check(
        "Engine present",
        has_make,
        if has_make {
            root_of(&layout)
        } else {
            "missing make/universal.mk — Prepare SDK".into()
        },
    );
    let bn = eng.join(".build_number").is_file();
    check(
        "Engine build number",
        bn,
        if bn {
            "ok".into()
        } else {
            "missing .build_number — re-prepare the engine".into()
        },
    );

    let host = native_host().unwrap_or_default();
    let host_bin = install::component_dir(&layout, Kind::Host, &host)
        .join("bin")
        .is_dir();
    check(
        "Host toolchain",
        host_bin,
        if host_bin {
            host.clone()
        } else {
            format!("{host} not installed")
        },
    );

    // The engine symlinks the toolchain store into itself; a stale link breaks
    // "no host specification found".
    let link = eng
        .join("toolchains")
        .join("hosts")
        .join(&host)
        .join("host.mk")
        .is_file();
    check(
        "Engine→toolchain link",
        link || !host_bin,
        if link {
            "resolves".into()
        } else {
            "broken — a build self-heals it".into()
        },
    );

    for tool in ["git", "make"] {
        let ok = has_command(tool);
        check(
            &format!("`{tool}` available"),
            ok,
            if ok {
                "on PATH".into()
            } else {
                "not found".into()
            },
        );
    }
    Ok(out)
}

// ---------- projects ----------

fn projects_path(layout: &Layout) -> std::path::PathBuf {
    layout.config.join("projects.json")
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectDto {
    name: String,
    path: String,
    engine: String,
    target: String,
    make_tool: String,
    created_at: String,
}

#[derive(Clone, Serialize)]
struct BuildLineDto {
    line: String,
}

fn project_dto(p: &Project) -> ProjectDto {
    ProjectDto {
        name: p.name.clone(),
        path: p.path.display().to_string(),
        engine: p.engine.clone(),
        target: p.target.clone(),
        make_tool: p.make_tool.clone(),
        created_at: p.created_at.clone(),
    }
}

#[tauri::command]
fn project_engines() -> Result<Vec<String>, String> {
    Ok(projects::installed_engines(&layout()?))
}

#[tauri::command]
fn list_projects() -> Result<Vec<ProjectDto>, String> {
    let layout = layout()?;
    let reg = ProjectRegistry::load(&projects_path(&layout)).map_err(|e| e.to_string())?;
    Ok(reg.projects.iter().map(project_dto).collect())
}

fn native_host() -> Result<String, String> {
    triple::host_triple_from(std::env::consts::ARCH, std::env::consts::OS)
        .map_err(|e| e.to_string())
}

/// Build readiness: installed targets, the native host, and whether the native
/// host toolchain is installed (you can't build without it). The UI uses this to
/// gate project creation and to pick the default target.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TargetsDto {
    targets: Vec<String>,
    host: String,
    host_installed: bool,
    /// Build tools available in the host toolchain (`xlmake`/`make`), preference
    /// order. One entry → no choice; two → the UI shows a selector.
    make_tools: Vec<String>,
}

#[tauri::command]
fn project_targets() -> Result<TargetsDto, String> {
    let layout = layout()?;
    let host = native_host()?;
    let host_bin = install::component_dir(&layout, Kind::Host, &host).join("bin");
    Ok(TargetsDto {
        targets: projects::installed_targets(&layout),
        host_installed: host_bin.is_dir(),
        make_tools: projects::available_make_tools(&host_bin),
        host,
    })
}

#[tauri::command]
fn create_project(
    location: String,
    name: String,
    engine: String,
    target: String,
    make_tool: String,
) -> Result<ProjectDto, String> {
    log::info!(
        "create_project name={name} location={location} engine={engine} target={target} make_tool={make_tool}"
    );
    // GNU make splits paths on whitespace, so a space anywhere in the project
    // path breaks every build — reject it up front (the UI guards this too).
    if location.contains(char::is_whitespace) {
        return Err("project location must not contain spaces".into());
    }
    let layout = layout()?;
    let engine_root = layout.engine_dir(&engine);
    if !engine_root.join("make").is_dir() {
        return Err(format!("engine '{engine}' is not installed"));
    }
    if !projects::is_valid_name(&name) {
        return Err(
            "project name must be non-empty and use only letters, digits, '-' or '_'".into(),
        );
    }
    let host = native_host()?;
    // A buildable project needs the host toolchain and the chosen target sysroot.
    if !install::component_dir(&layout, Kind::Host, &host)
        .join("bin")
        .is_dir()
    {
        return Err(format!("host toolchain '{host}' is not installed"));
    }
    if !install::component_dir(&layout, Kind::Target, &target).is_dir() {
        return Err(format!("target '{target}' is not installed"));
    }
    let host_bin = install::component_dir(&layout, Kind::Host, &host).join("bin");
    // Validate the chosen build tool against what the host toolchain actually
    // ships; default to the first available if the UI sent nothing.
    let available = projects::available_make_tools(&host_bin);
    let make_tool = if make_tool.is_empty() {
        available.first().cloned().unwrap_or_else(|| "make".into())
    } else if available.iter().any(|t| t == &make_tool) {
        make_tool
    } else {
        return Err(format!(
            "build tool '{make_tool}' is not in the host toolchain"
        ));
    };
    // The project lives in a new folder named after the project, inside `location`.
    let path = std::path::Path::new(&location).join(&name);
    projects::scaffold(&path, &name, &engine_root, &host, &host_bin, &make_tool)
        .map_err(|e| e.to_string())?;

    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let project = Project {
        name,
        path,
        engine,
        target,
        make_tool,
        created_at: now,
    };
    let pp = projects_path(&layout);
    let mut reg = ProjectRegistry::load(&pp).map_err(|e| e.to_string())?;
    reg.add(project.clone());
    reg.save(&pp).map_err(|e| e.to_string())?;
    Ok(project_dto(&project))
}

/// Switch an existing project's build tool: validate it against the host
/// toolchain, persist it to the registry, and rewrite the project's `.vscode`
/// config so VS Code drives the build with the chosen tool. Returns the updated
/// project.
#[tauri::command]
fn set_project_make_tool(path: String, make_tool: String) -> Result<ProjectDto, String> {
    log::info!("set_project_make_tool path={path} make_tool={make_tool}");
    let layout = layout()?;
    let host = native_host()?;
    let host_bin = install::component_dir(&layout, Kind::Host, &host).join("bin");
    if !projects::available_make_tools(&host_bin)
        .iter()
        .any(|t| t == &make_tool)
    {
        return Err(format!(
            "build tool '{make_tool}' is not in the host toolchain"
        ));
    }
    let pp = projects_path(&layout);
    let mut reg = ProjectRegistry::load(&pp).map_err(|e| e.to_string())?;
    let project = reg
        .projects
        .iter_mut()
        .find(|p| p.path.to_str() == Some(path.as_str()))
        .ok_or_else(|| "project not found".to_string())?;
    let engine_root = layout.engine_dir(&project.engine);
    projects::set_make_tool(
        &project.path,
        &project.name,
        &engine_root,
        &host,
        &host_bin,
        &make_tool,
    )
    .map_err(|e| e.to_string())?;
    project.make_tool = make_tool;
    let dto = project_dto(project);
    reg.save(&pp).map_err(|e| e.to_string())?;
    Ok(dto)
}

#[tauri::command]
fn remove_project(path: String) -> Result<(), String> {
    let layout = layout()?;
    let pp = projects_path(&layout);
    let mut reg = ProjectRegistry::load(&pp).map_err(|e| e.to_string())?;
    reg.remove(std::path::Path::new(&path));
    reg.save(&pp).map_err(|e| e.to_string())
}

// ---------- "open in editor" ----------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct EditorDto {
    id: &'static str,
    name: &'static str,
}

/// Whether `cmd` resolves on the user's PATH. On Windows we scan PATH/PATHEXT in
/// pure Rust (no subprocess → no console-window flash); elsewhere a GUI app's own
/// PATH is minimal (macOS especially), so resolve through a login shell.
#[cfg(target_os = "windows")]
fn has_command(cmd: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let exts = std::env::var("PATHEXT").unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into());
    std::env::split_paths(&path).any(|dir| {
        exts.split(';')
            .any(|ext| dir.join(format!("{cmd}{ext}")).is_file())
    })
}
#[cfg(not(target_os = "windows"))]
fn has_command(cmd: &str) -> bool {
    use std::path::PathBuf;
    // A Finder-launched GUI app inherits launchd's minimal PATH (`/usr/bin:/bin:…`),
    // and a non-interactive login shell can still miss CLIs whose PATH entry lives in
    // `.zshrc` or behind a tty guard — so a packaged build wouldn't see `claude`,
    // `code`, etc. Check the inherited PATH and the common user/brew bin dirs
    // DIRECTLY (no subprocess, no tty dependency), then fall back to a login shell.
    let on_path = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).any(|d| d.join(cmd).is_file()))
        .unwrap_or(false);
    if on_path {
        return true;
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(home) = std::env::var_os("HOME") {
        let h = PathBuf::from(home);
        for s in [
            ".local/bin",
            ".bun/bin",
            ".npm-global/bin",
            ".cargo/bin",
            ".deno/bin",
        ] {
            dirs.push(h.join(s));
        }
    }
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    if dirs.iter().any(|d| d.join(cmd).is_file()) {
        return true;
    }
    std::process::Command::new("zsh")
        .args(["-lc", &format!("command -v {cmd} >/dev/null 2>&1")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn app_installed(app: &str) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::Path::new(&format!("/Applications/{app}.app")).exists()
        || std::path::Path::new(&format!("{home}/Applications/{app}.app")).exists()
}
#[cfg(not(target_os = "macos"))]
fn app_installed(_: &str) -> bool {
    false
}

/// The install path of a GUI editor on Windows (user- and machine-wide installs),
/// since VS Code / Cursor are often not on PATH unless the user opted in.
#[cfg(target_os = "windows")]
fn win_editor_exe(id: &str) -> Option<std::path::PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA").map(std::path::PathBuf::from);
    let pf = std::env::var_os("ProgramFiles").map(std::path::PathBuf::from);
    let candidates: Vec<std::path::PathBuf> = match id {
        "vscode" => [
            local.as_ref().map(|l| {
                l.join("Programs")
                    .join("Microsoft VS Code")
                    .join("Code.exe")
            }),
            pf.as_ref()
                .map(|p| p.join("Microsoft VS Code").join("Code.exe")),
        ]
        .into_iter()
        .flatten()
        .collect(),
        "cursor" => [local
            .as_ref()
            .map(|l| l.join("Programs").join("cursor").join("Cursor.exe"))]
        .into_iter()
        .flatten()
        .collect(),
        _ => vec![],
    };
    candidates.into_iter().find(|p| p.is_file())
}

/// Native (beyond bare PATH) presence check for a GUI editor by id.
fn editor_present_natively(id: &str) -> bool {
    #[cfg(target_os = "macos")]
    {
        let app = match id {
            "vscode" => "Visual Studio Code",
            "cursor" => "Cursor",
            _ => return false,
        };
        app_installed(app)
    }
    #[cfg(target_os = "windows")]
    {
        win_editor_exe(id).is_some()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = id;
        false
    }
}

fn file_manager_name() -> &'static str {
    #[cfg(target_os = "macos")]
    {
        "Finder"
    }
    #[cfg(target_os = "windows")]
    {
        "Explorer"
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        "Files"
    }
}

/// The OS file manager (always available) followed by any detected editors.
#[tauri::command]
fn available_editors() -> Vec<EditorDto> {
    let mut out = vec![EditorDto {
        id: "files",
        name: file_manager_name(),
    }];
    if editor_present_natively("vscode") || has_command("code") {
        out.push(EditorDto {
            id: "vscode",
            name: "VS Code",
        });
    }
    if editor_present_natively("cursor") || has_command("cursor") {
        out.push(EditorDto {
            id: "cursor",
            name: "Cursor",
        });
    }
    if has_command("claude") {
        out.push(EditorDto {
            id: "claude",
            name: "Claude Code",
        });
    }
    out
}

#[tauri::command]
fn open_in_editor(path: String, editor: String) -> Result<(), String> {
    log::info!("open_in_editor editor={editor} path={path}");
    let spawn = |mut cmd: std::process::Command| -> Result<(), String> {
        clean_external_env(&mut cmd);
        cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
    };
    match editor.as_str() {
        "files" => spawn(file_manager_cmd(&path)),
        "vscode" => spawn(editor_open_cmd("code", "Visual Studio Code", &path)),
        "cursor" => spawn(editor_open_cmd("cursor", "Cursor", &path)),
        // Claude Code is a CLI — open a terminal in the project running `claude`.
        "claude" => spawn(claude_open_cmd(&path)),
        other => Err(format!("unknown editor: {other}")),
    }
}

/// Open the SDK working directory (`~/.local/share/xenolith`) — toolchains,
/// engines and the log file — in the OS file manager.
#[tauri::command]
fn open_working_dir() -> Result<(), String> {
    let dir = log_dir();
    let _ = std::fs::create_dir_all(&dir);
    log::info!("open_working_dir {}", dir.display());
    let mut cmd = file_manager_cmd(&dir.to_string_lossy());
    clean_external_env(&mut cmd);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Resolve a registered project plus its engine root and host toolchain `bin`.
fn project_paths(path: &str) -> Result<(Project, std::path::PathBuf, std::path::PathBuf), String> {
    let layout = layout()?;
    let project = ProjectRegistry::load(&projects_path(&layout))
        .map_err(|e| e.to_string())?
        .projects
        .iter()
        .find(|p| p.path.to_str() == Some(path))
        .cloned()
        .ok_or_else(|| "project not found".to_string())?;
    let engine_root = layout.engine_dir(&project.engine);
    let host = native_host()?;
    let host_bin = install::component_dir(&layout, Kind::Host, &host).join("bin");
    Ok((project, engine_root, host_bin))
}

/// Open a terminal in the project dir with `STAPPLER_ROOT` and the toolchain on
/// PATH already set — so `make` "just works" for hand-driving the build.
#[tauri::command]
fn open_terminal(path: String) -> Result<(), String> {
    log::info!("open_terminal path={path}");
    let (_p, engine_root, host_bin) = project_paths(&path)?;
    let root = projects::make_path(&engine_root);
    let bin = host_bin.display().to_string();
    let mut cmd = terminal_cmd(&path, &root, &bin);
    clean_external_env(&mut cmd);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Delete a project's `stappler-build/` output directory (clean).
#[tauri::command]
fn clean_project(path: String) -> Result<(), String> {
    log::info!("clean_project path={path}");
    let build = std::path::Path::new(&path).join("stappler-build");
    if build.is_dir() {
        std::fs::remove_dir_all(&build).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// AppImage's runtime prepends its bundled libraries/modules to a set of
/// environment variables. Any process we spawn (xdg-open, the file manager,
/// editors) then loads those bundled libs and dies with `undefined symbol`
/// (e.g. the AppImage's libcurl vs the system's nghttp2). Strip the AppDir
/// entries so spawned tools use the system libraries. No-op outside an AppImage.
#[cfg(target_os = "linux")]
fn clean_external_env(cmd: &mut std::process::Command) {
    let Some(appdir) = std::env::var_os("APPDIR") else {
        return;
    };
    let appdir = std::path::PathBuf::from(appdir);
    // Colon-separated path lists: keep only entries outside the AppDir.
    for var in [
        "LD_LIBRARY_PATH",
        "PATH",
        "GTK_PATH",
        "GST_PLUGIN_SYSTEM_PATH",
        "GST_PLUGIN_SYSTEM_PATH_1_0",
        "GIO_MODULE_DIR",
        "GIO_EXTRA_MODULES",
        "QT_PLUGIN_PATH",
        "GDK_PIXBUF_MODULEDIR",
        "GSETTINGS_SCHEMA_DIR",
        "PYTHONPATH",
        "PERLLIB",
        "XDG_DATA_DIRS",
        "XDG_CONFIG_DIRS",
    ] {
        let Some(val) = std::env::var_os(var) else {
            continue;
        };
        let kept: Vec<_> = std::env::split_paths(&val)
            .filter(|p| !p.starts_with(&appdir))
            .collect();
        match std::env::join_paths(&kept) {
            Ok(joined) if !kept.is_empty() => {
                cmd.env(var, joined);
            }
            _ => {
                cmd.env_remove(var);
            }
        }
    }
    // Single values pointing into the AppDir.
    for var in [
        "LD_PRELOAD",
        "GDK_PIXBUF_MODULE_FILE",
        "FONTCONFIG_FILE",
        "FONTCONFIG_PATH",
        "GTK_EXE_PREFIX",
        "PYTHONHOME",
    ] {
        let inside = std::env::var_os(var)
            .map(|v| std::path::Path::new(&v).starts_with(&appdir))
            .unwrap_or(false);
        if inside {
            cmd.env_remove(var);
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn clean_external_env(_cmd: &mut std::process::Command) {}

/// Reveal `path` in the OS file manager.
fn file_manager_cmd(path: &str) -> std::process::Command {
    #[cfg(target_os = "macos")]
    {
        let mut c = std::process::Command::new("open");
        c.arg(path);
        c
    }
    #[cfg(target_os = "windows")]
    {
        let mut c = std::process::Command::new("explorer");
        c.arg(path);
        c
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(path);
        c
    }
}

/// Open an interactive terminal at `path` with `STAPPLER_ROOT=<root>` exported and
/// `<bin>` prepended to PATH. Each OS sets the env inline in the launched shell
/// (terminal emulators often spawn the shell via a server that drops our env).
fn terminal_cmd(path: &str, root: &str, bin: &str) -> std::process::Command {
    #[cfg(target_os = "macos")]
    {
        let q = |s: &str| s.replace('\\', "\\\\").replace('"', "\\\"");
        let inner = format!(
            "cd '{}'; export STAPPLER_ROOT='{}'; export PATH='{}':\\\"$PATH\\\"; clear",
            path.replace('\'', "'\\''"),
            root.replace('\'', "'\\''"),
            bin.replace('\'', "'\\''"),
        );
        let script = format!(
            "tell application \"Terminal\"\nactivate\ndo script \"{}\"\nend tell",
            q(&inner)
        );
        let mut c = std::process::Command::new("osascript");
        c.args(["-e", &script]);
        c
    }
    #[cfg(target_os = "windows")]
    {
        let inner = format!(
            "cd /d \"{path}\" && set \"STAPPLER_ROOT={root}\" && set \"PATH={bin};%PATH%\""
        );
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", "cmd", "/K", &inner]);
        c
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let p = path.replace('\'', "'\\''");
        let r = root.replace('\'', "'\\''");
        let b = bin.replace('\'', "'\\''");
        // bash command run inside whichever terminal emulator is installed.
        let run = format!(
            "cd '{p}'; export STAPPLER_ROOT='{r}'; export PATH='{b}':\"$PATH\"; exec \"${{SHELL:-bash}}\""
        );
        // Try terminals in order; each gets the bash command via -e/--.
        let launch = format!(
            "if command -v konsole >/dev/null 2>&1; then exec konsole --workdir '{p}' -e bash -c \"$RUN\"; \
             elif command -v gnome-terminal >/dev/null 2>&1; then exec gnome-terminal --working-directory='{p}' -- bash -c \"$RUN\"; \
             elif command -v xfce4-terminal >/dev/null 2>&1; then exec xfce4-terminal --working-directory='{p}' -e \"bash -c '$RUN'\"; \
             elif command -v x-terminal-emulator >/dev/null 2>&1; then exec x-terminal-emulator -e bash -c \"$RUN\"; \
             elif command -v xterm >/dev/null 2>&1; then exec xterm -e bash -c \"$RUN\"; \
             else exit 1; fi"
        );
        let mut c = std::process::Command::new("sh");
        c.env("RUN", run).args(["-c", &launch]);
        c
    }
}

/// Open a GUI editor at `path`. macOS prefers LaunchServices (`open -a`, no PATH
/// issues); Windows launches the resolved `.exe` (no console flash) and falls back
/// to the CLI shim; elsewhere the CLI via a login shell.
fn editor_open_cmd(cli: &str, app: &str, path: &str) -> std::process::Command {
    #[cfg(target_os = "macos")]
    {
        if app_installed(app) {
            let mut c = std::process::Command::new("open");
            c.args(["-a", app, path]);
            return c;
        }
        let mut c = std::process::Command::new("zsh");
        c.args(["-lc", &format!("{cli} \"$1\""), "_", path]);
        c
    }
    #[cfg(target_os = "windows")]
    {
        let _ = app;
        let id = if cli == "code" { "vscode" } else { "cursor" };
        if let Some(exe) = win_editor_exe(id) {
            let mut c = std::process::Command::new(exe);
            c.arg(path);
            return c;
        }
        // `code`/`cursor` on PATH are .cmd shims — launch them via cmd.
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", cli, path]);
        c
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = app;
        let mut c = std::process::Command::new("zsh");
        c.args(["-lc", &format!("{cli} \"$1\""), "_", path]);
        c
    }
}

fn claude_open_cmd(path: &str) -> std::process::Command {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "tell application \"Terminal\"\nactivate\ndo script \"cd '{}' && claude\"\nend tell",
            path.replace('\'', "'\\''")
        );
        let mut c = std::process::Command::new("osascript");
        c.args(["-e", &script]);
        c
    }
    #[cfg(target_os = "windows")]
    {
        // Open a fresh console in the project dir running the claude CLI.
        let mut c = std::process::Command::new("cmd");
        c.args(["/C", "start", "", "/D"])
            .arg(path)
            .args(["cmd", "/K", "claude"]);
        c
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let mut c = std::process::Command::new("claude");
        c.current_dir(path);
        c
    }
}

#[tauri::command]
async fn build_project(
    app: AppHandle,
    path: String,
    target: String,
    run: bool,
) -> Result<i32, String> {
    tauri::async_runtime::spawn_blocking(move || build_blocking(&app, &path, &target, run))
        .await
        .map_err(|e| e.to_string())?
}

/// Build a project for `target` (and optionally run it, only when `target` is the
/// native host), streaming output as `build://line` events. Returns the exit code.
fn build_blocking(app: &AppHandle, path: &str, target: &str, run: bool) -> Result<i32, String> {
    log::info!("build_project path={path} target={target} run={run}");
    let layout = layout()?;
    // Heal toolchain links before building: older installs symlinked the store with
    // ABSOLUTE paths, so moving the data root (`~/.xenolith` → `~/.local/share/…`)
    // left them dangling ("no host specification found"). Relinking rewrites them as
    // relative paths to the current store.
    let _ = install::relink_all_engines(&layout);
    let reg = ProjectRegistry::load(&projects_path(&layout)).map_err(|e| e.to_string())?;
    let project = reg
        .projects
        .iter()
        .find(|p| p.path.to_str() == Some(path))
        .ok_or_else(|| "project not found".to_string())?
        .clone();

    // Belt-and-suspenders for line endings: a project scaffolded by an older
    // build (or edited on Windows) can have a CRLF Makefile, and a stray `\r`
    // folds into make variables and SPLITS the compile command so it won't run.
    // Strip CR from the Makefile before every build so old projects self-heal.
    let makefile = std::path::Path::new(path).join("Makefile");
    if let Ok(content) = std::fs::read_to_string(&makefile) {
        if content.contains('\r') {
            log::info!("normalizing CRLF -> LF in {}", makefile.display());
            let _ = std::fs::write(&makefile, content.replace('\r', ""));
        }
    }

    let engine_root = layout.engine_dir(&project.engine);
    let host = native_host()?;
    let host_bin = install::component_dir(&layout, Kind::Host, &host).join("bin");
    if !host_bin.is_dir() {
        return Err(format!("host toolchain '{host}' is not installed"));
    }
    // Building for a target needs that target's sysroot installed — without it the
    // engine aborts with "TARGET_SYSROOT is not defined".
    if !install::component_dir(&layout, Kind::Target, target).is_dir() {
        return Err(format!("target '{target}' is not installed"));
    }
    // Put the toolchain's compilers/make on PATH and point at STAPPLER_ROOT.
    // On Windows also guarantee PowerShell is on PATH: the engine's build recipes
    // (buildconfig, shaders) are PowerShell-only, and `make` falls back to cmd.exe
    // if it can't locate `powershell.exe`, which silently breaks them.
    let mut path_dirs: Vec<std::path::PathBuf> = vec![host_bin];
    #[cfg(target_os = "windows")]
    let powershell_exe = {
        let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into());
        let ps_dir = std::path::PathBuf::from(&sysroot)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0");
        let ps_exe = ps_dir.join("powershell.exe");
        path_dirs.push(ps_dir);
        ps_exe
    };
    path_dirs.extend(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    ));
    let path_env = std::env::join_paths(path_dirs).map_err(|e| e.to_string())?;

    // Native build: the default goal (host → all) builds the .app AND has the
    // engine generate Contents/Info.plist, so it runs in place. Cross build: pass
    // STAPPLER_TARGET (can't run the result here anyway). Build number comes from
    // the `.build_number` files baked into the bundle.
    // Build in parallel — settings override, else one job per logical CPU.
    // Without -j make runs single-threaded (every step serially).
    let jobs = load_settings()
        .jobs
        .filter(|n| *n >= 1)
        .unwrap_or_else(auto_jobs);
    // A target with the same base triple as the host (e.g. `<host>+sprt`) is the
    // native arch and CAN run here; only a different-arch target is a true cross.
    let target_base = target.split('+').next().unwrap_or(target);
    let runnable = target_base == host;

    // STAPPLER_ROOT must use forward slashes: this env value overrides the
    // Makefile's default, and GNU make breaks on Windows backslash paths.
    // Drive the build with the tool the project was created with (`xlmake`/`make`)
    // so the GUI build matches the VS Code `makefile.makePath`. Legacy projects
    // with no recorded tool fall back to GNU make. Both resolve via PATH (the host
    // toolchain `bin/` is prepended above); on Windows `Command` appends `.exe`.
    let tool = if project.make_tool.is_empty() {
        "make"
    } else {
        project.make_tool.as_str()
    };
    let mut make = std::process::Command::new(tool);
    make.current_dir(path)
        .arg(format!("-j{jobs}"))
        .env("STAPPLER_ROOT", projects::make_path(&engine_root))
        .env("PATH", &path_env)
        // Force the C locale so GNU make's gettext messages come out in ASCII
        // English instead of the localized console codepage (e.g. CP1251 on a
        // Russian Windows), which otherwise garbles the captured build log.
        .env("LC_ALL", "C")
        .env("LANG", "C");
    // Windows: the engine selects PowerShell recipes (OS=Windows_NT) but `make`
    // often can't honor the makefile's `SHELL = powershell.exe` and falls back to
    // cmd.exe, so PowerShell-only steps (buildconfig, shaders) break. Override
    // SHELL with the absolute powershell path (forward slashes — GNU make dislikes
    // backslashes) so the recipes actually run under PowerShell.
    #[cfg(target_os = "windows")]
    if powershell_exe.is_file() {
        make.arg(format!(
            "SHELL={}",
            powershell_exe.to_string_lossy().replace('\\', "/")
        ));
    }
    if !runnable {
        // True cross-compile: `install` the artifacts (can't run them here).
        make.arg("install").arg(format!("STAPPLER_TARGET={target}"));
    } else if target != host {
        // Native variant (e.g. +sprt): the DEFAULT goal builds the runnable .app
        // WITH Contents/Info.plist (so the Vulkan loader finds Frameworks) —
        // `make install` would skip the plist. Just select the target.
        make.arg(format!("STAPPLER_TARGET={target}"));
    }
    let started = std::time::Instant::now();
    let code = stream_cmd(app, &mut make)?;
    // Report build wall-clock to the console (e.g. "✓ Built in 3m 21s").
    let secs = started.elapsed().as_secs();
    let dur = if secs >= 60 {
        format!("{}m {}s", secs / 60, secs % 60)
    } else {
        format!("{secs}s")
    };
    let mark = if code == 0 {
        "✓ Built"
    } else {
        "✗ Build failed"
    };
    let _ = app.emit(
        "build://line",
        BuildLineDto {
            line: format!("{mark} in {dur} (exit {code})"),
        },
    );
    // Run any native build (host or a same-arch variant like +sprt); a true
    // cross-compiled binary won't run on this host.
    if code != 0 || !run || !runnable {
        return Ok(code);
    }

    // Run the binary inside the `.app` bundle: with Contents/Info.plist present
    // the engine resolves the Vulkan loader from Contents/Frameworks. cwd = the
    // project so bundled `resources/…` resolve.
    let exe_name = projects::sanitize_name(&project.name);
    let out_dir = std::path::Path::new(path)
        .join("stappler-build")
        .join(target)
        .join("debug")
        .join(projects::host_cc_subdir());
    let candidates = [
        out_dir.join(format!("{exe_name}.app/Contents/MacOS/{exe_name}")), // macOS bundle
        out_dir.join(format!("{exe_name}.exe")),                           // Windows
        out_dir.join(&exe_name),                                           // Linux
    ];
    let exe = candidates
        .iter()
        .find(|p| p.exists())
        .cloned()
        .unwrap_or_else(|| out_dir.join(&exe_name));

    // macOS: launch the `.app` through LaunchServices (`open`), NOT by exec'ing
    // Contents/MacOS/<bin> directly. A directly-exec'd bundle runs as a background
    // process: it creates its NSWindow/Metal layer (the swapchain logs appear) but
    // never activates, so the window never comes to the front. `open` registers it
    // as a foreground GUI app and brings the window up.
    #[cfg(target_os = "macos")]
    {
        if let Some(bundle) = exe
            .ancestors()
            .find(|p| p.extension().is_some_and(|e| e == "app"))
        {
            return run_macos_bundle(app, bundle, &exe, path);
        }
    }

    let mut runner = std::process::Command::new(&exe);
    runner.current_dir(path);
    stream_cmd(app, &mut runner)
}

/// Launch a macOS `.app` via `open` so it activates and shows its window, while
/// still streaming the app's stdout/stderr into the build console. `open -W` waits
/// for the app to exit; its output is redirected to a log file we tail meanwhile.
///
/// If LaunchServices refuses (`open` prints `failed with error -10810`), fall back
/// to exec'ing the binary directly so the app still runs.
#[cfg(target_os = "macos")]
fn run_macos_bundle(
    app: &AppHandle,
    bundle: &std::path::Path,
    exe: &std::path::Path,
    cwd: &str,
) -> Result<i32, String> {
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::atomic::Ordering;

    let build_dir = std::path::Path::new(cwd).join("stappler-build");
    let _ = std::fs::create_dir_all(&build_dir);
    let log_path = build_dir.join("run.log");
    let err_path = build_dir.join("run-open.err");
    // Truncate any previous run's output.
    let _ = std::fs::write(&log_path, b"");
    let _ = std::fs::write(&err_path, b"");

    let _ = app.emit(
        "build://line",
        BuildLineDto {
            line: format!("▶ Launching {}", bundle.display()),
        },
    );
    log::info!("run: launching {} via `open`", bundle.display());

    let open_err = std::fs::File::create(&err_path).map_err(|e| e.to_string())?;
    let mut child = std::process::Command::new("open")
        .arg("-W") // wait until the app exits
        .arg("-n") // always a fresh instance
        .arg("--stdout")
        .arg(&log_path)
        .arg("--stderr")
        .arg(&log_path)
        .arg(bundle)
        .current_dir(cwd)
        // A GUI app passes its `__CFBundleIdentifier` to children; if `open`
        // inherits ours, LaunchServices misidentifies the caller and fails with
        // -10810. Drop it so `open` runs in a clean LS context.
        .env_remove("__CFBundleIdentifier")
        .stderr(open_err) // `open`'s own messages (the app's go to --stderr)
        .spawn()
        .map_err(|e| e.to_string())?;
    BUILD_PID.store(child.id(), Ordering::Relaxed);

    // Tail the redirected log until `open` (and thus the app) exits.
    let mut pos: u64 = 0;
    let mut carry = String::new();
    let pump_log = |pos: &mut u64, carry: &mut String| {
        let Ok(mut f) = std::fs::File::open(&log_path) else {
            return;
        };
        if f.seek(SeekFrom::Start(*pos)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        if f.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
            *pos += buf.len() as u64;
            carry.push_str(&String::from_utf8_lossy(&buf));
            while let Some(nl) = carry.find('\n') {
                let line: String = carry.drain(..=nl).collect();
                let line = strip_ansi(line.trim_end_matches(['\n', '\r']));
                log::info!("> {line}");
                let _ = app.emit("build://line", BuildLineDto { line });
            }
        }
    };

    let status = loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) => break status,
            None => {
                pump_log(&mut pos, &mut carry);
                std::thread::sleep(std::time::Duration::from_millis(150));
            }
        }
    };
    pump_log(&mut pos, &mut carry); // flush the tail
    if !carry.is_empty() {
        let line = strip_ansi(carry.trim_end_matches(['\n', '\r']));
        let _ = app.emit("build://line", BuildLineDto { line });
    }
    BUILD_PID.store(0, Ordering::Relaxed);
    log::info!("run: `open` exited (code {:?})", status.code());

    // `open` writes to its own stderr ONLY when something went wrong (e.g.
    // LaunchServices -10810, or a TCC/automation permission denial). Any content
    // there means the app almost certainly never launched — log the reason and
    // fall back to exec'ing the binary directly: a plain fork/exec that bypasses
    // LaunchServices (and its TCC gating) entirely, so the app still starts and we
    // stream its output live.
    let ls_err = std::fs::read_to_string(&err_path).unwrap_or_default();
    if !ls_err.trim().is_empty() {
        log::error!("run: `open` failed: {}", ls_err.trim());
        let _ = app.emit(
            "build://line",
            BuildLineDto {
                line: format!(
                    "⚠ `open` could not launch the app ({}). Launching the binary directly…",
                    ls_err.trim()
                ),
            },
        );
        let mut runner = std::process::Command::new(exe);
        runner.current_dir(cwd);
        return stream_cmd(app, &mut runner);
    }
    Ok(status.code().unwrap_or(0))
}

/// Remove ANSI CSI escape sequences (`ESC [ … <letter>`) from a line.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for n in chars.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Stream a reader line by line into `emit`, reading RAW BYTES and converting
/// lossily. We must NOT use `BufRead::lines()`: it yields an error on the first
/// non-UTF-8 byte and the usual `map_while(Result::ok)` then DROPS the entire
/// rest of the stream — fatal on a localized Windows where the compiler emits
/// CP1251 (so the actual build errors, which come late, vanished from the log).
fn pump<R: std::io::Read>(r: R, emit: &dyn Fn(String)) {
    use std::io::{BufRead, BufReader};
    let mut reader = BufReader::new(r);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        match reader.read_until(b'\n', &mut buf) {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                while matches!(buf.last(), Some(b'\n' | b'\r')) {
                    buf.pop();
                }
                emit(String::from_utf8_lossy(&buf).into_owned());
            }
        }
    }
}

fn stream_cmd(app: &AppHandle, cmd: &mut std::process::Command) -> Result<i32, String> {
    use std::sync::atomic::Ordering;
    log::info!("exec: {cmd:?}");
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // Own process group so `cancel_build` can kill the whole build tree (make +
    // the compilers it spawns), not just the top process.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| {
        log::error!("spawn failed: {e}");
        e.to_string()
    })?;
    BUILD_PID.store(child.id(), Ordering::Relaxed);
    let out = child.stdout.take();
    let err = child.stderr.take();
    // A toolchain binary that aborts with SIGILL/SIGSEGV (e.g. a prebuilt clang
    // compiled for a newer CPU baseline than this machine) shows up only as a
    // cryptic "Illegal instruction" + "Error 132" deep in make's output. Flag it
    // so we can append a plain-language hint instead of leaving the user staring
    // at a raw crash dump.
    let toolchain_crashed = std::sync::atomic::AtomicBool::new(false);
    let emit = |line: String| {
        // The engine colorizes its output; strip ANSI so both the UI console and
        // the log file are plain text.
        let line = strip_ansi(&line);
        if line.contains("Illegal instruction")
            || line.contains("Bad CPU type")
            || (line.contains("Segmentation fault") && line.contains("/bin/"))
        {
            toolchain_crashed.store(true, Ordering::Relaxed);
        }
        log::info!("> {line}");
        let _ = app.emit("build://line", BuildLineDto { line });
    };
    std::thread::scope(|s| {
        if let Some(out) = out {
            s.spawn(|| pump(out, &emit));
        }
        if let Some(err) = err {
            s.spawn(|| pump(err, &emit));
        }
    });
    let status = child.wait().map_err(|e| e.to_string())?;
    BUILD_PID.store(0, Ordering::Relaxed);
    let code = status.code().unwrap_or(-1);
    log::info!("exit: {code}");
    if code != 0 && toolchain_crashed.load(Ordering::Relaxed) {
        let hint = "⚠ The compiler crashed (Illegal instruction). The installed \
            toolchain is likely incompatible with this CPU — please report this to \
            the Xenolith maintainers with your Mac model.";
        log::error!("{hint}");
        let _ = app.emit(
            "build://line",
            BuildLineDto {
                line: hint.to_string(),
            },
        );
    }
    Ok(code)
}

/// Kill the running build (and its process group). No-op if nothing is building.
#[tauri::command]
fn cancel_build() -> Result<(), String> {
    let pid = BUILD_PID.swap(0, std::sync::atomic::Ordering::Relaxed);
    if pid == 0 {
        return Ok(());
    }
    log::info!("cancel_build pid={pid}");
    #[cfg(unix)]
    {
        // Negative target = the whole process group (make + compilers).
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .status();
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("taskkill")
            .args(["/T", "/F", "/PID", &pid.to_string()])
            .status();
    }
    Ok(())
}

/// Where the rolling log file lives: the SDK root (`~/.local/share/xenolith/installer.log`),
/// so testers can just send it. Falls back to the temp dir.
fn log_dir() -> std::path::PathBuf {
    layout()
        .ok()
        .and_then(|l| l.config.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(std::env::temp_dir)
}

// ---------- self-update (Tauri updater, GitHub releases) ----------

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDto {
    /// Version offered by the release manifest.
    version: String,
    /// Version currently running.
    current_version: String,
    /// Release notes, if the manifest carries them.
    notes: Option<String>,
    /// Publish date (RFC 3339), if present.
    date: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateProgressDto {
    downloaded: u64,
    /// Total bytes, if the server sent a content length.
    total: Option<u64>,
}

/// Check the configured GitHub release endpoint for a newer signed build.
/// `Ok(None)` means we are up to date.
#[tauri::command]
async fn check_update(app: AppHandle) -> Result<Option<UpdateDto>, String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    match updater.check().await.map_err(|e| e.to_string())? {
        Some(u) => Ok(Some(UpdateDto {
            version: u.version.clone(),
            current_version: u.current_version.clone(),
            notes: u.body.clone(),
            date: u.date.and_then(|d| d.format(&Rfc3339).ok()),
        })),
        None => Ok(None),
    }
}

/// Download + verify (minisign) + install the latest update, streaming bytes as
/// `update://progress`, then relaunch into the new version. Does not return on
/// success (the app restarts).
#[tauri::command]
async fn install_update(app: AppHandle) -> Result<(), String> {
    let updater = app.updater().map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "no update available".to_string())?;
    log::info!("install_update -> {}", update.version);

    let app2 = app.clone();
    let mut downloaded: u64 = 0;
    update
        .download_and_install(
            move |chunk, total| {
                downloaded += chunk as u64;
                let _ = app2.emit("update://progress", UpdateProgressDto { downloaded, total });
            },
            || log::info!("update download finished, installing"),
        )
        .await
        .map_err(|e| e.to_string())?;

    // Relaunch into the freshly installed version (diverges — never returns).
    app.restart();
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log::LevelFilter::Info)
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Folder {
                        path: log_dir(),
                        file_name: Some("installer".into()),
                    }),
                ])
                .build(),
        )
        .setup(|_app| {
            log::info!(
                "Xenolith Installer {} starting ({} {}), data: {}",
                env!("CARGO_PKG_VERSION"),
                std::env::consts::OS,
                std::env::consts::ARCH,
                log_dir().display()
            );
            Ok(())
        })
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            detect_host,
            translate,
            load_catalog,
            install_component,
            uninstall_component,
            engine_status,
            prepare_engine,
            project_engines,
            project_targets,
            list_projects,
            create_project,
            set_project_make_tool,
            remove_project,
            build_project,
            available_editors,
            open_in_editor,
            open_working_dir,
            open_terminal,
            clean_project,
            install_for_system,
            disk_usage,
            remove_engine,
            project_size,
            get_settings,
            set_settings,
            set_data_dir,
            cancel_build,
            diagnostics_report,
            run_doctor,
            check_update,
            install_update
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Xenolith installer");
}
