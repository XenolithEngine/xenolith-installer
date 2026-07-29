//! Headless CLI front-end. Thin shell over `xenolith-installer-core`:
//! parse args, build the real context (FTP transport, PGP verifier, resolved
//! layout, locale), and dispatch to [`commands::run`].

mod commands;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

use xenolith_installer_core::{
    dirs::Layout,
    i18n::I18n,
    key_source, releases,
    transport_ftp::FtpTransport,
    verify::{AcceptAll, PgpVerifier, RejectAll, Verifier},
};

use commands::{run, Command, Ctx};

/// Parent directory under which each release (`sdk-v…`) lives on the server.
const RELEASES_ROOT: &str = "/releases";
/// Last-resort release when discovery fails (offline / blocked listing).
const FALLBACK_RELEASE: &str = "sdk-v0alpha0";

/// Resolve the remote base dir and release name. Explicit `--base`/`--release` win;
/// otherwise **discover the latest release on the server** (like the GUI) so the CLI
/// never gets stuck on a stale hardcoded release.
fn resolve_base_release(
    transport: &FtpTransport,
    base: Option<String>,
    release: Option<String>,
) -> (String, String) {
    match (base, release) {
        (Some(b), Some(r)) => (b, r),
        (None, Some(r)) => (format!("{RELEASES_ROOT}/{r}"), r),
        (Some(b), None) => {
            let r = b
                .rsplit('/')
                .find(|s| !s.is_empty())
                .unwrap_or(&b)
                .to_string();
            (b, r)
        }
        (None, None) => match releases::latest_release(transport, RELEASES_ROOT, 4) {
            Ok(Some(rel)) => (rel.base(RELEASES_ROOT), rel.name),
            _ => (
                format!("{RELEASES_ROOT}/{FALLBACK_RELEASE}"),
                FALLBACK_RELEASE.to_string(),
            ),
        },
    }
}

#[derive(Parser)]
#[command(
    name = "xenolith-installer-cli",
    about = "Install and manage the Xenolith Engine SDK",
    version
)]
struct Cli {
    /// UI language (e.g. `en`, `ru`). Defaults to the system locale.
    #[arg(long, global = true)]
    lang: Option<String>,
    /// Install prefix override (otherwise `$XENOLITH_HOME` or OS default).
    #[arg(long, global = true)]
    prefix: Option<PathBuf>,
    /// Use a local engine checkout as `STAPPLER_ROOT` instead of the baked
    /// bundle (also: `$XENOLITH_ENGINE`, or Settings → Engine path).
    #[arg(long, global = true)]
    engine: Option<PathBuf>,
    /// Release server `host:port`.
    #[arg(long, global = true, default_value = "stappler.dev:21")]
    server: String,
    /// Remote directory holding `hosts/` and `targets/` (default: the latest release).
    #[arg(long, global = true)]
    base: Option<String>,
    /// SDK catalogue release id (e.g. `sdk-v0beta0`). Not the app build mode —
    /// that is `build --release`. Named `--sdk-release` so it does not clash with
    /// clap's access of the build flag.
    #[arg(long = "sdk-release", visible_alias = "catalog-release", global = true)]
    sdk_release: Option<String>,
    /// DEV ONLY: skip signature verification. Never use for real installs.
    #[arg(long, global = true)]
    insecure_accept_unsigned: bool,
    #[command(subcommand)]
    command: Sub,
}

#[derive(Subcommand)]
enum Sub {
    /// Print the detected native host triple.
    Detect,
    /// List the catalogue with install status.
    List,
    /// Install by id: `engine` for just the engine bundle, a triple for one
    /// toolchain component — or, with NO id, provision the whole SDK for this
    /// machine (engine + native host toolchain + native target + `+sprt`).
    Install {
        /// `engine`, or a target/host triple. Omit to provision the whole system.
        id: Option<String>,
        /// Install the HOST toolchain for the triple (it may be both host and target).
        #[arg(long)]
        host: bool,
        /// Install the TARGET sysroot for the triple.
        #[arg(long)]
        target: bool,
    },
    /// Scaffold a new project named <name> (in --path, default: current dir).
    New {
        /// Project name (letters, digits, '-' or '_').
        name: String,
        /// Parent directory to create the project in.
        #[arg(long, default_value = ".")]
        path: String,
    },
    /// Build a project directory, optionally running it afterwards.
    Build {
        /// Path to the project (the folder with its Makefile).
        path: String,
        /// Build target triple (default: the native host).
        #[arg(long)]
        target: Option<String>,
        /// Run the built binary afterwards (native targets only).
        #[arg(long)]
        run: bool,
        /// Build optimized (release: -O2 -DNDEBUG, no debug symbols) instead of debug.
        #[arg(long)]
        release: bool,
    },
    /// Validate the installed-state registry against the filesystem.
    Verify,
    /// Show components for which a newer release exists.
    Update,
    /// Update this installer binary itself from the latest GitHub release.
    SelfUpdate,
}

impl From<Sub> for Command {
    fn from(s: Sub) -> Self {
        match s {
            Sub::Detect => Command::Detect,
            Sub::List => Command::List,
            Sub::Install { id, host, target } => Command::Install { id, host, target },
            Sub::New { name, path } => Command::New {
                name,
                location: path,
            },
            Sub::Build {
                path,
                target,
                run,
                release,
            } => Command::Build {
                path,
                target,
                run,
                release,
            },
            Sub::Verify => Command::Verify,
            Sub::Update => Command::Update,
            // Intercepted in `main` before dispatch (it needs no FTP/key context).
            Sub::SelfUpdate => unreachable!("self-update is handled before dispatch"),
        }
    }
}

/// Replace this binary with the newest GitHub release asset for the host target.
fn run_self_update() -> ExitCode {
    match self_update::backends::github::Update::configure()
        .repo_owner("XenolithEngine")
        .repo_name("xenolith-installer")
        .bin_name("xenolith-installer-cli")
        .current_version(env!("CARGO_PKG_VERSION"))
        .show_download_progress(true)
        .build()
        .and_then(|u| u.update())
    {
        Ok(self_update::Status::UpToDate(v)) => {
            println!("already up to date ({v})");
            ExitCode::SUCCESS
        }
        Ok(self_update::Status::Updated(v)) => {
            println!("updated to {v}; re-run to use the new version");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: self-update failed: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // Self-update needs neither the FTP transport nor the signing key, so handle
    // it before building that context.
    if matches!(cli.command, Sub::SelfUpdate) {
        return run_self_update();
    }

    let layout = match Layout::resolve_from_env(cli.prefix.as_deref()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::FAILURE;
        }
    };

    let i18n = match &cli.lang {
        Some(l) => I18n::new(l),
        None => I18n::from_env(),
    };

    let transport = FtpTransport::new(cli.server.clone());

    // Only `install` verifies signatures. Fetch and pin the release key just for
    // that case; other commands get an unused RejectAll so a keyserver outage
    // never blocks `list`/`detect`/`verify`/`update`.
    let verifier: Box<dyn Verifier> = if cli.insecure_accept_unsigned {
        eprintln!("warning: signature verification disabled (--insecure-accept-unsigned)");
        Box::new(AcceptAll)
    } else if matches!(cli.command, Sub::Install { .. }) {
        match key_source::fetch_release_key().and_then(|asc| {
            PgpVerifier::release(&asc).map_err(|e| {
                key_source::KeyFetchError::Http(format!("key did not match the pin: {e}"))
            })
        }) {
            Ok(v) => Box::new(v),
            Err(e) => {
                eprintln!("error: could not establish a trusted signing key: {e}");
                eprintln!(
                    "hint: re-run with --insecure-accept-unsigned only if you trust the source"
                );
                return ExitCode::FAILURE;
            }
        }
    } else {
        Box::new(RejectAll)
    };

    let now = OffsetDateTime::now_utc()
        .format(&Rfc3339)
        .unwrap_or_else(|_| "unknown".to_string());

    // Only the catalogue commands hit the release; resolve it (discovering the
    // latest release when not overridden) just for those to avoid a needless FTP
    // listing on `detect`/`new`/`build`/`verify`.
    let (remote_base, release) =
        if matches!(cli.command, Sub::List | Sub::Install { .. } | Sub::Update) {
            resolve_base_release(&transport, cli.base.clone(), cli.sdk_release.clone())
        } else {
            (
                cli.base
                    .clone()
                    .unwrap_or_else(|| format!("{RELEASES_ROOT}/{FALLBACK_RELEASE}")),
                cli.sdk_release
                    .clone()
                    .unwrap_or_else(|| FALLBACK_RELEASE.into()),
            )
        };

    let ctx = Ctx {
        transport: &transport,
        verifier: verifier.as_ref(),
        layout,
        i18n,
        remote_base,
        release,
        now,
        arch: std::env::consts::ARCH.to_string(),
        os: std::env::consts::OS.to_string(),
        engine: cli.engine.clone(),
    };

    match run(&cli.command.into(), &ctx) {
        Ok(out) => {
            print!("{out}");
            if !out.ends_with('\n') {
                println!();
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
