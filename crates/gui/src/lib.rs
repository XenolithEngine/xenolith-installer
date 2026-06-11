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
    engine_source::{EngineBundle, EngineInfo},
    i18n::I18n,
    install::{self, Installer, Phase},
    key_source,
    manifest::{self, Kind},
    projects::{self, Project, ProjectRegistry},
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
/// Engine bundle ref to install as STAPPLER_ROOT (temporary GH-release source).
const ENGINE_REF: &str = "master";

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
    phase: &'static str,
    bytes: u64,
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
    install::uninstall(&layout, kind, id).map_err(|e| e.to_string())?;
    let path = layout.installed_manifest();
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
    let mut last_step = u64::MAX;
    let info = EngineBundle::new(ENGINE_REF)
        .install(layout, &mut |bytes| {
            // Throttle to ~one event per 256 KiB downloaded.
            let step = bytes / (256 * 1024);
            if step != last_step {
                last_step = step;
                let _ = app.emit(
                    "engine://progress",
                    EngineProgressDto {
                        phase: "downloading",
                        bytes,
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
    // Toolchains install into <engine>/toolchains, so the engine (STAPPLER_ROOT)
    // must exist first. Downloads it on first use; no-op afterwards.
    ensure_engine(app, &layout, false)?;
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

    // Link the freshly-installed toolchain into every installed engine.
    install::relink_all_engines(&layout).map_err(|e| e.to_string())?;
    Ok(())
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
}

#[tauri::command]
fn project_targets() -> Result<TargetsDto, String> {
    let layout = layout()?;
    let host = native_host()?;
    Ok(TargetsDto {
        targets: projects::installed_targets(&layout),
        host_installed: install::component_dir(&layout, Kind::Host, &host)
            .join("bin")
            .is_dir(),
        host,
    })
}

#[tauri::command]
fn create_project(
    location: String,
    name: String,
    engine: String,
    target: String,
) -> Result<ProjectDto, String> {
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
    // The project lives in a new folder named after the project, inside `location`.
    let path = std::path::Path::new(&location).join(&name);
    projects::scaffold(&path, &name, &engine_root, &host, &host_bin).map_err(|e| e.to_string())?;

    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_default();
    let project = Project {
        name,
        path,
        engine,
        target,
        created_at: now,
    };
    let pp = projects_path(&layout);
    let mut reg = ProjectRegistry::load(&pp).map_err(|e| e.to_string())?;
    reg.add(project.clone());
    reg.save(&pp).map_err(|e| e.to_string())?;
    Ok(project_dto(&project))
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

/// Whether `cmd` resolves on the user's login PATH (a GUI app's own PATH is
/// minimal on macOS, so go through a login shell).
fn has_command(cmd: &str) -> bool {
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
    if app_installed("Visual Studio Code") || has_command("code") {
        out.push(EditorDto {
            id: "vscode",
            name: "VS Code",
        });
    }
    if app_installed("Cursor") || has_command("cursor") {
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
    let spawn = |mut cmd: std::process::Command| -> Result<(), String> {
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

/// Open a GUI editor at `path`: macOS prefers LaunchServices (`open -a`, no PATH
/// issues); elsewhere the CLI via a login shell.
fn editor_open_cmd(cli: &str, app: &str, path: &str) -> std::process::Command {
    #[cfg(target_os = "macos")]
    if app_installed(app) {
        let mut c = std::process::Command::new("open");
        c.args(["-a", app, path]);
        return c;
    }
    let _ = app;
    let mut c = std::process::Command::new("zsh");
    c.args(["-lc", &format!("{cli} \"$1\"",), "_", path]);
    c
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
    #[cfg(not(target_os = "macos"))]
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
    let layout = layout()?;
    let reg = ProjectRegistry::load(&projects_path(&layout)).map_err(|e| e.to_string())?;
    let project = reg
        .projects
        .iter()
        .find(|p| p.path.to_str() == Some(path))
        .ok_or_else(|| "project not found".to_string())?
        .clone();

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
    let path_env = std::env::join_paths(std::iter::once(host_bin).chain(std::env::split_paths(
        &std::env::var_os("PATH").unwrap_or_default(),
    )))
    .map_err(|e| e.to_string())?;

    // Native build: the default goal (host → all) builds the .app AND has the
    // engine generate Contents/Info.plist, so it runs in place. Cross build: pass
    // STAPPLER_TARGET (can't run the result here anyway). Build number comes from
    // the `.build_number` files baked into the bundle.
    let mut make = std::process::Command::new("make");
    make.current_dir(path)
        .env("STAPPLER_ROOT", &engine_root)
        .env("PATH", &path_env);
    if target != host {
        make.arg("install").arg(format!("STAPPLER_TARGET={target}"));
    }
    let code = stream_cmd(app, &mut make)?;
    // Run only a native build — a cross-compiled binary won't run on this host.
    if code != 0 || !run || target != host {
        return Ok(code);
    }

    // Run the binary inside the `.app` bundle: with Contents/Info.plist present
    // the engine resolves the Vulkan loader from Contents/Frameworks. cwd = the
    // project so bundled `resources/…` resolve.
    let exe_name = projects::sanitize_name(&project.name);
    let out_dir = std::path::Path::new(path)
        .join("stappler-build")
        .join(target)
        .join("debug/cc");
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
    let mut runner = std::process::Command::new(&exe);
    runner.current_dir(path);
    stream_cmd(app, &mut runner)
}

fn stream_cmd(app: &AppHandle, cmd: &mut std::process::Command) -> Result<i32, String> {
    use std::io::{BufRead, BufReader};
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let out = child.stdout.take();
    let err = child.stderr.take();
    std::thread::scope(|s| {
        if let Some(out) = out {
            s.spawn(move || {
                for line in BufReader::new(out).lines().map_while(Result::ok) {
                    let _ = app.emit("build://line", BuildLineDto { line });
                }
            });
        }
        if let Some(err) = err {
            s.spawn(move || {
                for line in BufReader::new(err).lines().map_while(Result::ok) {
                    let _ = app.emit("build://line", BuildLineDto { line });
                }
            });
        }
    });
    let status = child.wait().map_err(|e| e.to_string())?;
    Ok(status.code().unwrap_or(-1))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
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
            remove_project,
            build_project,
            available_editors,
            open_in_editor
        ])
        .run(tauri::generate_context!())
        .expect("error while running the Xenolith installer");
}
