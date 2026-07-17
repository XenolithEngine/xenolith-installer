//! Command logic, decoupled from `clap` and from the real network so it can be
//! unit-tested with a mock transport. `main.rs` builds the real context (FTP
//! transport, PGP verifier, resolved layout) and calls [`run`].

use xenolith_installer_core::{
    catalog::{build_catalog, promote_native, Status},
    dirs::Layout,
    i18n::{group_label, I18n},
    install::{self, component_dir, Installer, Phase},
    manifest::{self, Kind, Manifest},
    projects, provision,
    state::{InstalledComponent, InstalledState},
    transport::Transport,
    triple::{self, resolve_host},
    verify::Verifier,
};

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error(transparent)]
    Transport(#[from] xenolith_installer_core::transport::TransportError),
    #[error(transparent)]
    Install(#[from] xenolith_installer_core::install::InstallError),
    #[error(transparent)]
    State(#[from] xenolith_installer_core::state::StateError),
    #[error("{0}")]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Print the detected native host triple.
    Detect,
    /// List the catalogue with install status.
    List,
    /// Install by id (`engine`, or a triple with `--host`/`--target`), or — with
    /// no id — provision the whole SDK: engine + native host + native target (+`+sprt`).
    Install {
        id: Option<String>,
        host: bool,
        target: bool,
    },
    /// Scaffold a new project: `new <name>` in `location` (default: cwd).
    New { name: String, location: String },
    /// Build a project directory (optionally run it afterwards).
    Build {
        path: String,
        target: Option<String>,
        run: bool,
    },
    /// Validate the installed-state registry against the filesystem.
    Verify,
    /// Show components for which a newer release exists.
    Update,
}

/// Everything a command needs, injected so tests can supply a mock transport.
pub struct Ctx<'a> {
    pub transport: &'a dyn Transport,
    pub verifier: &'a dyn Verifier,
    pub layout: Layout,
    pub i18n: I18n,
    /// Remote directory holding `hosts/` and `targets/`, e.g. `/releases/sdk-v0alpha0`.
    pub remote_base: String,
    pub release: String,
    /// RFC 3339 timestamp recorded on install (front-end supplies the clock).
    pub now: String,
    /// Native arch/os, injected for testability (real `main` uses `std::env::consts`).
    pub arch: String,
    pub os: String,
}

impl Ctx<'_> {
    fn state_path(&self) -> std::path::PathBuf {
        self.layout.installed_manifest()
    }
}

/// Fetch and assemble the remote manifest, retrying the flaky listing.
fn fetch_manifest(ctx: &Ctx) -> Result<(Manifest, Vec<String>), CliError> {
    Ok(manifest::fetch_manifest(
        ctx.transport,
        &ctx.remote_base,
        &ctx.release,
        4,
    )?)
}

pub fn run(cmd: &Command, ctx: &Ctx) -> Result<String, CliError> {
    match cmd {
        Command::Detect => detect(ctx),
        Command::List => list(ctx),
        // `install engine` is the engine bundle only; a triple (disambiguated by
        // `--host`/`--target`) is one component; no id provisions the whole system.
        Command::Install {
            id: Some(id),
            host,
            target,
        } if id == "engine" && !host && !target => install_engine(ctx),
        Command::Install {
            id: Some(id),
            host,
            target,
        } => install(ctx, id, *host, *target),
        Command::Install { id: None, .. } => provision(ctx),
        Command::New { name, location } => new_project(ctx, name, location),
        Command::Build { path, target, run } => build(ctx, path, target.as_deref(), *run),
        Command::Verify => verify(ctx),
        Command::Update => update(ctx),
    }
}

fn detect(ctx: &Ctx) -> Result<String, CliError> {
    match resolve_host(&ctx.arch, &ctx.os).map_err(|e| CliError::Other(e.to_string()))? {
        Some(h) if h.via_emulation => Ok(format!("{} (host via {})", h.native, h.host_archive)),
        Some(h) => Ok(h.native),
        None => Ok(format!("no SDK host available for {}-{}", ctx.arch, ctx.os)),
    }
}

fn native_id(ctx: &Ctx) -> Option<String> {
    triple::host_triple_from(&ctx.arch, &ctx.os).ok()
}

fn list(ctx: &Ctx) -> Result<String, CliError> {
    let (manifest, dropped) = fetch_manifest(ctx)?;
    let state = InstalledState::load(&ctx.state_path())?;
    let mut rows = build_catalog(&manifest, &state);
    if let Some(native) = native_id(ctx) {
        promote_native(&mut rows, &native);
    }

    // An empty catalogue almost always means the fetch came back empty rather than
    // the server genuinely having nothing — most often because the release server
    // still uses plain FTP, whose passive-mode data connections many networks and
    // firewalls silently block/mangle. Surface that instead of printing bare group
    // headers with no components (which reads as "nothing to install").
    if rows.is_empty() {
        return Err(CliError::Other(format!(
            "the component catalogue from '{}{}' came back empty.\n\
             The release server currently uses plain FTP, which many home/office \
             networks, VPNs and firewalls block or mangle (the login succeeds but the \
             passive data connection returns nothing). Nothing was downloaded.\n\
             Try a different network, or check back once the HTTPS catalogue is live.",
            ctx.remote_base, ctx.release,
        )));
    }

    let mut out = String::new();
    for kind in [Kind::Target, Kind::Host] {
        out.push_str(&group_label(&ctx.i18n, kind));
        out.push('\n');
        for row in rows.iter().filter(|r| r.kind == kind) {
            let status = match &row.status {
                Status::Installed => ctx.i18n.get("status-installed"),
                Status::NotInstalled => ctx.i18n.get("status-not-installed"),
                Status::UpdateAvailable { latest_release, .. } => ctx
                    .i18n
                    .get_args("status-update-available", &[("version", latest_release)]),
            };
            out.push_str(&format!("  {:<40} {}\n", row.id, status));
        }
    }
    if !dropped.is_empty() {
        out.push_str(&format!(
            "\n[skipped {} unsigned artifact(s): {}]\n",
            dropped.len(),
            dropped.join(", ")
        ));
    }
    Ok(out)
}

fn kind_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Host => "host toolchain",
        Kind::Target => "target",
    }
}

/// Install one toolchain component by triple. The same triple can be both a host
/// toolchain and a target sysroot, so `--host`/`--target` disambiguate; with
/// neither flag we install the sole match, or error if it's ambiguous.
fn install(ctx: &Ctx, id: &str, want_host: bool, want_target: bool) -> Result<String, CliError> {
    let (manifest, _) = fetch_manifest(ctx)?;
    let has_host = manifest.find_kind(id, Kind::Host).is_some();
    let has_target = manifest.find_kind(id, Kind::Target).is_some();

    let kind = if want_host {
        if !has_host {
            return Err(CliError::Other(format!(
                "no host toolchain '{id}' in the catalogue"
            )));
        }
        Kind::Host
    } else if want_target {
        if !has_target {
            return Err(CliError::Other(format!(
                "no target '{id}' in the catalogue"
            )));
        }
        Kind::Target
    } else {
        match (has_host, has_target) {
            (true, true) => {
                return Err(CliError::Other(format!(
                    "'{id}' exists as both a host toolchain and a target — pass --host or --target"
                )))
            }
            (true, false) => Kind::Host,
            (false, true) => Kind::Target,
            (false, false) => return Err(CliError::Other(format!("unknown component: {id}"))),
        }
    };

    let installer = Installer {
        transport: ctx.transport,
        verifier: ctx.verifier,
        layout: &ctx.layout,
        remote_base: ctx.remote_base.clone(),
        release: ctx.release.clone(),
    };
    install_component_named(ctx, &installer, &manifest, id, kind, kind_label(kind))?;
    Ok(format!(
        "{} {id} ({})",
        ctx.i18n.get("status-installed"),
        kind_label(kind)
    ))
}

/// `install` with no id: provision the whole SDK for this machine — the engine
/// bundle, the native host toolchain, and the native target plus its `+sprt`
/// variant (the CLI equivalent of the GUI's "install everything for my system").
fn provision(ctx: &Ctx) -> Result<String, CliError> {
    let host = native_id(ctx)
        .ok_or_else(|| CliError::Other(format!("no SDK host for {}-{}", ctx.arch, ctx.os)))?;

    let short = ensure_engine_cli(ctx)?;

    let (manifest, _) = fetch_manifest(ctx)?;
    let installer = Installer {
        transport: ctx.transport,
        verifier: ctx.verifier,
        layout: &ctx.layout,
        remote_base: ctx.remote_base.clone(),
        release: ctx.release.clone(),
    };

    install_component_named(
        ctx,
        &installer,
        &manifest,
        &host,
        Kind::Host,
        "host toolchain",
    )?;
    if manifest.find_kind(&host, Kind::Target).is_some() {
        install_component_named(ctx, &installer, &manifest, &host, Kind::Target, "target")?;
    }
    let sprt = format!("{host}+sprt");
    if manifest.find_kind(&sprt, Kind::Target).is_some() {
        install_component_named(
            ctx,
            &installer,
            &manifest,
            &sprt,
            Kind::Target,
            "target (+sprt)",
        )?;
    }

    Ok(format!(
        "SDK ready for {host} (engine {short}).\n\
         Next: `xenolith-installer-cli new <name>`, then `build <name> --run`."
    ))
}

/// `install engine`: download just the engine bundle (no toolchains).
fn install_engine(ctx: &Ctx) -> Result<String, CliError> {
    let short = ensure_engine_cli(ctx)?;
    Ok(format!(
        "engine {short} installed.\n\
         Add toolchains with `install <triple>`, or `install` to provision everything."
    ))
}

/// Ensure the engine bundle is present, streaming coarse download progress to
/// stderr. Returns the short version hash. Shared by `install` and `install engine`.
fn ensure_engine_cli(ctx: &Ctx) -> Result<String, CliError> {
    eprintln!("• Engine ({})", provision::ENGINE_REF);
    let mut last = u64::MAX;
    let info = provision::ensure_engine(&ctx.layout, false, &mut |bytes, total| {
        let step = bytes / (512 * 1024);
        if step != last {
            last = step;
            match total {
                Some(t) if t > 0 => eprint!(
                    "\r    {:>3.0}%  ({} / {} MB)   ",
                    bytes as f64 / t as f64 * 100.0,
                    bytes / 1_000_000,
                    t / 1_000_000
                ),
                _ => eprint!("\r    {} MB   ", bytes / 1_000_000),
            }
        }
    })
    .map_err(CliError::Other)?;
    eprintln!("\r    \u{2713} engine {}                    ", info.short());
    Ok(info.short())
}

/// Download + verify + extract the component with `id` of the given `kind`,
/// streaming coarse download progress, and record it in the install registry.
fn install_component_named(
    ctx: &Ctx,
    installer: &Installer,
    manifest: &Manifest,
    id: &str,
    kind: Kind,
    label: &str,
) -> Result<(), CliError> {
    let component = manifest
        .find_kind(id, kind)
        .ok_or_else(|| CliError::Other(format!("{label} '{id}' is not in the catalogue")))?;
    let total = component.size;
    eprintln!("• {label}: {id}");
    let mut last = u64::MAX;
    let record = installer.install_component(component, &ctx.now, &mut |phase, bytes| {
        if matches!(phase, Phase::Downloading) && total > 0 {
            let step = bytes / (512 * 1024);
            if step != last {
                last = step;
                eprint!(
                    "\r    {:>3.0}%  ({} / {} MB)   ",
                    bytes as f64 / total as f64 * 100.0,
                    bytes / 1_000_000,
                    total / 1_000_000
                );
            }
        }
    })?;
    eprintln!("\r    \u{2713} {}                         ", record.id);
    let mut state = InstalledState::load(&ctx.state_path())?;
    state.upsert(record);
    state.save(&ctx.state_path())?;
    Ok(())
}

/// Scaffold a new project directory named `name` inside `location`.
fn new_project(ctx: &Ctx, name: &str, location: &str) -> Result<String, CliError> {
    if !projects::is_valid_name(name) {
        return Err(CliError::Other(
            "project name must use only letters, digits, '-' or '_' (no spaces)".into(),
        ));
    }
    if location.contains(char::is_whitespace) {
        return Err(CliError::Other(
            "project location must not contain spaces (GNU make breaks on them)".into(),
        ));
    }
    let host = native_id(ctx)
        .ok_or_else(|| CliError::Other(format!("no SDK host for {}-{}", ctx.arch, ctx.os)))?;
    let engine_root = ctx.layout.engine_dir(provision::ENGINE_REF);
    if !engine_root.join("make/universal.mk").is_file() {
        return Err(CliError::Other(
            "engine not installed — run `xenolith-installer-cli install` first".into(),
        ));
    }
    let host_bin = component_dir(&ctx.layout, Kind::Host, &host).join("bin");
    if !host_bin.is_dir() {
        return Err(CliError::Other(format!(
            "host toolchain '{host}' not installed — run `install` first"
        )));
    }
    let make_tool = projects::available_make_tools(&host_bin)
        .first()
        .cloned()
        .unwrap_or_else(|| projects::default_make_tool().to_string());
    let path = std::path::Path::new(location).join(name);
    projects::scaffold(&path, name, &engine_root, &host, &host_bin, &make_tool)
        .map_err(|e| CliError::Other(e.to_string()))?;
    Ok(format!(
        "created project at {}\nBuild it: xenolith-installer-cli build {} --run",
        path.display(),
        path.display()
    ))
}

/// Build a project directory with the SDK toolchain; optionally run the result.
fn build(ctx: &Ctx, path: &str, target: Option<&str>, run: bool) -> Result<String, CliError> {
    let proj = std::path::Path::new(path);
    if !proj.join("Makefile").is_file() {
        return Err(CliError::Other(format!(
            "no Makefile in {}",
            proj.display()
        )));
    }
    let host = native_id(ctx)
        .ok_or_else(|| CliError::Other(format!("no SDK host for {}-{}", ctx.arch, ctx.os)))?;
    let target = target.map(str::to_string).unwrap_or_else(|| host.clone());
    let engine_root = ctx.layout.engine_dir(provision::ENGINE_REF);
    let host_bin = component_dir(&ctx.layout, Kind::Host, &host).join("bin");
    if !host_bin.is_dir() {
        return Err(CliError::Other(format!(
            "host toolchain '{host}' not installed"
        )));
    }
    if !component_dir(&ctx.layout, Kind::Target, &target).is_dir() {
        return Err(CliError::Other(format!("target '{target}' not installed")));
    }
    // Heal stale toolchain symlinks (e.g. after a data-root move) before building.
    let _ = install::relink_all_engines(&ctx.layout);

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
    let path_env = std::env::join_paths(path_dirs).map_err(|e| CliError::Other(e.to_string()))?;

    let jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let target_base = target.split('+').next().unwrap_or(&target);
    let runnable = target_base == host;

    let mut make = std::process::Command::new("make");
    make.current_dir(proj)
        .arg(format!("-j{jobs}"))
        .env("STAPPLER_ROOT", projects::make_path(&engine_root))
        .env("PATH", &path_env)
        .env("LC_ALL", "C")
        .env("LANG", "C");
    #[cfg(target_os = "windows")]
    if powershell_exe.is_file() {
        make.arg(format!(
            "SHELL={}",
            powershell_exe.to_string_lossy().replace('\\', "/")
        ));
    }
    if !runnable {
        make.arg("install").arg(format!("STAPPLER_TARGET={target}"));
    } else if target != host {
        make.arg(format!("STAPPLER_TARGET={target}"));
    }
    eprintln!("• Building {} for {target} (-j{jobs})…", proj.display());
    let status = make.status().map_err(|e| CliError::Other(e.to_string()))?;
    let code = status.code().unwrap_or(-1);
    if code != 0 {
        return Err(CliError::Other(format!("build failed (exit {code})")));
    }

    if run && !runnable {
        return Ok(format!(
            "built {} for {target} — cross-compiled, cannot run on this host",
            proj.display()
        ));
    }
    if run {
        return run_built(proj, &target);
    }
    Ok(format!("built {} for {target}", proj.display()))
}

/// Run a freshly-built project binary. The CLI runs from a terminal (with full
/// session/WindowServer access), so a direct exec shows the window fine — no
/// LaunchServices dance is needed (unlike the packaged GUI).
fn run_built(proj: &std::path::Path, target: &str) -> Result<String, CliError> {
    let name = proj
        .file_name()
        .map(|s| projects::sanitize_name(&s.to_string_lossy()))
        .ok_or_else(|| CliError::Other("bad project path".into()))?;
    let out_dir = proj
        .join("stappler-build")
        .join(target)
        .join("debug")
        .join(projects::host_cc_subdir());
    let candidates = [
        out_dir.join(format!("{name}.app/Contents/MacOS/{name}")),
        out_dir.join(format!("{name}.exe")),
        out_dir.join(&name),
    ];
    let exe = candidates
        .iter()
        .find(|p| p.exists())
        .ok_or_else(|| CliError::Other("built binary not found".into()))?;
    eprintln!("▶ running {}", exe.display());
    let status = std::process::Command::new(exe)
        .current_dir(proj)
        .status()
        .map_err(|e| CliError::Other(e.to_string()))?;
    Ok(format!(
        "ran {} (exit {})",
        exe.display(),
        status.code().unwrap_or(-1)
    ))
}

fn verify(ctx: &Ctx) -> Result<String, CliError> {
    let state = InstalledState::load(&ctx.state_path())?;
    let invalid: Vec<&InstalledComponent> = state.invalid(|p| p.exists());
    if invalid.is_empty() {
        Ok(format!("{} components OK", state.components.len()))
    } else {
        let ids: Vec<&str> = invalid.iter().map(|c| c.id.as_str()).collect();
        Ok(format!("INVALID: {}", ids.join(", ")))
    }
}

fn update(ctx: &Ctx) -> Result<String, CliError> {
    let (manifest, _) = fetch_manifest(ctx)?;
    let state = InstalledState::load(&ctx.state_path())?;
    let rows = build_catalog(&manifest, &state);
    let updatable: Vec<&str> = rows
        .iter()
        .filter(|r| matches!(r.status, Status::UpdateAvailable { .. }))
        .map(|r| r.id.as_str())
        .collect();
    if updatable.is_empty() {
        Ok("up to date".to_string())
    } else {
        Ok(format!("updates: {}", updatable.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use xenolith_installer_core::extract::testing::make_tar_xz;
    use xenolith_installer_core::transport::testing::MockTransport;
    use xenolith_installer_core::verify::AcceptAll;

    fn ctx_with<'a>(
        transport: &'a MockTransport,
        verifier: &'a AcceptAll,
        home: &std::path::Path,
        arch: &str,
        os: &str,
    ) -> Ctx<'a> {
        Ctx {
            transport,
            verifier,
            layout: Layout::from_home(home),
            i18n: I18n::new("en"),
            remote_base: "/releases/sdk-v0alpha0".into(),
            release: "sdk-v0alpha0".into(),
            now: "2026-06-09T00:00:00Z".into(),
            arch: arch.into(),
            os: os.into(),
        }
    }

    fn linux_archive() -> Vec<u8> {
        make_tar_xz(&[("bin/xenolith", b"ELF", true)])
    }

    fn transport_with_linux(archive: &[u8]) -> MockTransport {
        let hosts = format!(
            "-rw-r--r-- 1 0 0 {} Jun 08 19:39 x86_64-unknown-linux-gnu.tar.xz\n\
             -rw-r--r-- 1 0 0 3 Jun 08 19:40 x86_64-unknown-linux-gnu.tar.xz.sig",
            archive.len()
        );
        MockTransport::new()
            .with_listing("/releases/sdk-v0alpha0/hosts/", &hosts)
            .with_listing("/releases/sdk-v0alpha0/targets/", "")
            .with_file(
                "/releases/sdk-v0alpha0/hosts/x86_64-unknown-linux-gnu.tar.xz",
                archive,
            )
            .with_file(
                "/releases/sdk-v0alpha0/hosts/x86_64-unknown-linux-gnu.tar.xz.sig",
                b"sig",
            )
    }

    #[test]
    fn detect_reports_native_host() {
        let t = MockTransport::new();
        let v = AcceptAll;
        let home = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&t, &v, home.path(), "x86_64", "linux");
        assert_eq!(
            run(&Command::Detect, &ctx).unwrap(),
            "x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn detect_reports_emulation_fallback() {
        // Windows-on-ARM runs the x86_64 host under emulation.
        let t = MockTransport::new();
        let v = AcceptAll;
        let home = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&t, &v, home.path(), "aarch64", "windows");
        let out = run(&Command::Detect, &ctx).unwrap();
        assert!(out.contains("x86_64-pc-windows-msvc"), "got: {out}");
    }

    #[test]
    fn detect_intel_mac_is_native_host() {
        // Intel Macs have their own x86_64 host — no arm fallback.
        let t = MockTransport::new();
        let v = AcceptAll;
        let home = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&t, &v, home.path(), "x86_64", "macos");
        let out = run(&Command::Detect, &ctx).unwrap();
        assert!(out.contains("x86_64-apple-macosx"), "got: {out}");
        assert!(!out.contains("aarch64"), "got: {out}");
    }

    #[test]
    fn list_shows_not_installed_then_install_flips_to_installed() {
        let archive = linux_archive();
        let t = transport_with_linux(&archive);
        let v = AcceptAll;
        let home = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&t, &v, home.path(), "x86_64", "linux");

        let before = run(&Command::List, &ctx).unwrap();
        assert!(before.contains("x86_64-unknown-linux-gnu"));
        assert!(before.contains("Not Installed"));

        let msg = run(
            &Command::Install {
                id: Some("x86_64-unknown-linux-gnu".into()),
                host: false,
                target: false,
            },
            &ctx,
        )
        .unwrap();
        assert!(msg.contains("Installed"));
        // Files placed and registry updated.
        assert!(ctx
            .layout
            .toolchains_store_dir()
            .join("hosts/x86_64-unknown-linux-gnu/bin/xenolith")
            .exists());

        let after = run(&Command::List, &ctx).unwrap();
        assert!(after.contains("Installed"));
        assert!(!after.contains("Not Installed"));
    }

    #[test]
    fn install_unknown_component_errors() {
        let archive = linux_archive();
        let t = transport_with_linux(&archive);
        let v = AcceptAll;
        let home = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&t, &v, home.path(), "x86_64", "linux");
        assert!(run(
            &Command::Install {
                id: Some("no-such".into()),
                host: false,
                target: false,
            },
            &ctx
        )
        .is_err());
    }

    #[test]
    fn verify_reports_ok_for_empty_state() {
        let t = MockTransport::new();
        let v = AcceptAll;
        let home = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&t, &v, home.path(), "x86_64", "linux");
        assert!(run(&Command::Verify, &ctx).unwrap().contains("OK"));
    }
}
