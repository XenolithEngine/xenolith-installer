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
    key_source,
    transport_ftp::FtpTransport,
    verify::{AcceptAll, PgpVerifier, RejectAll, Verifier},
};

use commands::{run, Command, Ctx};

#[derive(Parser)]
#[command(
    name = "xenolith-installer",
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
    /// Release server `host:port`.
    #[arg(long, global = true, default_value = "stappler.dev:21")]
    server: String,
    /// Remote directory holding `hosts/` and `targets/`.
    #[arg(long, global = true, default_value = "/releases/sdk-v0alpha0")]
    base: String,
    /// Release identifier.
    #[arg(long, global = true, default_value = "sdk-v0alpha0")]
    release: String,
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
    /// Install a component by id (triple, optionally with `+variant`).
    Install { id: String },
    /// Validate the installed-state registry against the filesystem.
    Verify,
    /// Show components for which a newer release exists.
    Update,
}

impl From<Sub> for Command {
    fn from(s: Sub) -> Self {
        match s {
            Sub::Detect => Command::Detect,
            Sub::List => Command::List,
            Sub::Install { id } => Command::Install { id },
            Sub::Verify => Command::Verify,
            Sub::Update => Command::Update,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();

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

    let ctx = Ctx {
        transport: &transport,
        verifier: verifier.as_ref(),
        layout,
        i18n,
        remote_base: cli.base.clone(),
        release: cli.release.clone(),
        now,
        arch: std::env::consts::ARCH.to_string(),
        os: std::env::consts::OS.to_string(),
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
