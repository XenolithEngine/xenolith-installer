//! Command logic, decoupled from `clap` and from the real network so it can be
//! unit-tested with a mock transport. `main.rs` builds the real context (FTP
//! transport, PGP verifier, resolved layout) and calls [`run`].

use xenolith_installer_core::{
    catalog::{build_catalog, promote_native, Status},
    dirs::Layout,
    i18n::{group_label, I18n},
    install::Installer,
    manifest::{self, Component, Kind, Manifest},
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
    /// Install a component by id (triple, optionally with +variant).
    Install { id: String },
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
        Command::Install { id } => install(ctx, id),
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

fn find_component(manifest: &Manifest, id: &str) -> Option<Component> {
    manifest.find(id).cloned()
}

fn install(ctx: &Ctx, id: &str) -> Result<String, CliError> {
    let (manifest, _) = fetch_manifest(ctx)?;
    if find_component(&manifest, id).is_none() {
        return Err(CliError::Other(format!("unknown component: {id}")));
    }
    let installer = Installer {
        transport: ctx.transport,
        verifier: ctx.verifier,
        layout: &ctx.layout,
        remote_base: ctx.remote_base.clone(),
        release: ctx.release.clone(),
    };
    let record = installer.install(&manifest, id, &ctx.now, &mut |_, _| {})?;

    let mut state = InstalledState::load(&ctx.state_path())?;
    state.upsert(record.clone());
    state.save(&ctx.state_path())?;
    Ok(format!(
        "{} {}",
        ctx.i18n.get("status-installed"),
        record.id
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
        let t = MockTransport::new();
        let v = AcceptAll;
        let home = tempfile::tempdir().unwrap();
        let ctx = ctx_with(&t, &v, home.path(), "x86_64", "macos");
        let out = run(&Command::Detect, &ctx).unwrap();
        assert!(out.contains("aarch64-apple-macosx"), "got: {out}");
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
                id: "x86_64-unknown-linux-gnu".into(),
            },
            &ctx,
        )
        .unwrap();
        assert!(msg.contains("Installed"));
        // Files placed and registry updated.
        assert!(ctx
            .layout
            .sdk_root()
            .join("sdk-v0alpha0/hosts/x86_64-unknown-linux-gnu/bin/xenolith")
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
                id: "no-such".into()
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
