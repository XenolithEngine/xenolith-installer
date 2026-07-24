//! Target-triple detection and mapping.
//!
//! The Xenolith SDK on the FTP names every artifact with an LLVM/Rust-style
//! target triple: `<arch>-<vendor>-<os>[-<abi>][+<variant>]`. We must map the
//! current platform (and, crucially, its *native* arch under emulation) to the
//! server's naming, which differs from Rust's in one place: macOS is
//! `apple-macosx`, NOT `apple-darwin`.

use std::fmt;

/// Host triples for which a host toolchain archive exists on the FTP
/// (`/releases/<rel>/hosts/`), mirroring the `sdk-v0beta1` host set. This is the
/// OFFLINE fast-path used by `detect`; the authoritative list is the fetched
/// manifest, which `install`/`provision` resolve against — so keep this in sync
/// but never treat it as the source of truth (it has drifted twice already).
/// `aarch64`/`riscv64` Windows have no native host and fall back — see
/// [`host_fallback`].
pub const KNOWN_HOSTS: &[&str] = &[
    "aarch64-apple-macosx",
    "x86_64-apple-macosx",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "riscv64-unknown-linux-gnu",
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "riscv64-unknown-linux-musl",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TripleError {
    #[error("unsupported OS for SDK host: {0}")]
    UnsupportedOs(String),
    #[error("unsupported architecture for SDK host: {0}")]
    UnsupportedArch(String),
}

/// Map a Rust `std::env::consts::OS` value (plus the Linux libc flavour) to the
/// server's vendor-os segment. `libc` is consulted only for Linux
/// (`Some("musl")` → `unknown-linux-musl`, otherwise `unknown-linux-gnu`).
pub fn server_os(os: &str, libc: Option<&str>) -> Result<String, TripleError> {
    Ok(match os {
        "macos" | "ios" => "apple-macosx".to_string(),
        "windows" => "pc-windows-msvc".to_string(),
        "linux" | "android" => format!("unknown-linux-{}", libc.unwrap_or("gnu")),
        other => return Err(TripleError::UnsupportedOs(other.to_string())),
    })
}

/// Normalise a Rust `std::env::consts::ARCH` value to the server's arch segment.
pub fn server_arch(arch: &str) -> Result<&'static str, TripleError> {
    Ok(match arch {
        "aarch64" | "arm64" => "aarch64",
        "x86_64" | "amd64" => "x86_64",
        "riscv64" => "riscv64",
        other => return Err(TripleError::UnsupportedArch(other.to_string())),
    })
}

/// Build the server triple for an (arch, os) pair, defaulting Linux to glibc.
/// Pure — for the libc-aware, machine-sensing version use [`native_host_triple`].
pub fn host_triple_from(arch: &str, os: &str) -> Result<String, TripleError> {
    host_triple_from_libc(arch, os, None)
}

/// Build the server triple for an (arch, os, libc) triple. Pure.
pub fn host_triple_from_libc(
    arch: &str,
    os: &str,
    libc: Option<&str>,
) -> Result<String, TripleError> {
    Ok(format!("{}-{}", server_arch(arch)?, server_os(os, libc)?))
}

/// The current machine's Linux libc (`"gnu"`/`"musl"`), or `None` off Linux.
/// A glibc clang won't run on a musl-only box (Alpine) and vice-versa, so this
/// decides which host toolchain the machine can actually execute.
pub fn current_libc(os: &str) -> Option<&'static str> {
    if os != "linux" {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        Some(detect_linux_libc())
    }
    #[cfg(not(target_os = "linux"))]
    {
        Some("gnu")
    }
}

#[cfg(target_os = "linux")]
fn detect_linux_libc() -> &'static str {
    use std::path::Path;
    // musl installs its dynamic loader as /lib/ld-musl-<arch>.so.1 and Alpine
    // adds /etc/alpine-release; a glibc system has neither.
    let musl = Path::new("/lib/ld-musl-aarch64.so.1").exists()
        || Path::new("/lib/ld-musl-x86_64.so.1").exists()
        || Path::new("/lib/ld-musl-riscv64.so.1").exists()
        || Path::new("/etc/alpine-release").exists();
    if musl {
        "musl"
    } else {
        "gnu"
    }
}

/// The server host triple for the running machine, libc-aware on Linux.
pub fn native_host_triple(arch: &str, os: &str) -> Result<String, TripleError> {
    host_triple_from_libc(arch, os, current_libc(os))
}

/// When no host toolchain exists for `triple`, pick the host the current
/// machine can run via emulation (win-arm64 → x64 host under WOW64). Returns
/// `None` when nothing can run it.
pub fn host_fallback(triple: &str) -> Option<&'static str> {
    if let Some(h) = KNOWN_HOSTS.iter().find(|h| **h == triple) {
        // Caller should normally check this first, but be forgiving.
        return Some(*h);
    }
    match triple {
        // Windows on ARM runs x86_64 binaries via emulation.
        "aarch64-pc-windows-msvc" => Some("x86_64-pc-windows-msvc"),
        _ => None,
    }
}

/// A resolved host: the triple the machine *is*, plus the host archive triple
/// we will actually download (identical unless a fallback kicked in).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedHost {
    pub native: String,
    pub host_archive: String,
    pub via_emulation: bool,
}

impl fmt::Display for ResolvedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.via_emulation {
            write!(f, "{} (via {})", self.native, self.host_archive)
        } else {
            write!(f, "{}", self.native)
        }
    }
}

/// Resolve a native (arch, os) into a downloadable host, applying the fallback
/// policy. `Ok(None)` means the platform is valid but no host can run on it.
pub fn resolve_host(arch: &str, os: &str) -> Result<Option<ResolvedHost>, TripleError> {
    let native = native_host_triple(arch, os)?;
    if KNOWN_HOSTS.contains(&native.as_str()) {
        return Ok(Some(ResolvedHost {
            host_archive: native.clone(),
            native,
            via_emulation: false,
        }));
    }
    Ok(host_fallback(&native).map(|h| ResolvedHost {
        native,
        host_archive: h.to_string(),
        via_emulation: true,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_uses_apple_macosx_not_darwin() {
        // THE trap: server names macOS `apple-macosx`, Rust would say `apple-darwin`.
        assert_eq!(
            host_triple_from("aarch64", "macos").unwrap(),
            "aarch64-apple-macosx"
        );
        assert!(!host_triple_from("aarch64", "macos")
            .unwrap()
            .contains("darwin"));
    }

    #[test]
    fn windows_and_linux_hosts() {
        assert_eq!(
            host_triple_from("x86_64", "windows").unwrap(),
            "x86_64-pc-windows-msvc"
        );
        assert_eq!(
            host_triple_from("x86_64", "linux").unwrap(),
            "x86_64-unknown-linux-gnu"
        );
    }

    #[test]
    fn ios_maps_to_macosx_and_android_to_linux() {
        assert_eq!(server_os("ios", None).unwrap(), "apple-macosx");
        assert_eq!(server_os("android", None).unwrap(), "unknown-linux-gnu");
    }

    #[test]
    fn linux_libc_selects_gnu_or_musl() {
        assert_eq!(
            host_triple_from_libc("aarch64", "linux", Some("musl")).unwrap(),
            "aarch64-unknown-linux-musl"
        );
        assert_eq!(
            host_triple_from_libc("aarch64", "linux", Some("gnu")).unwrap(),
            "aarch64-unknown-linux-gnu"
        );
        // No libc info → default to glibc.
        assert_eq!(
            host_triple_from_libc("aarch64", "linux", None).unwrap(),
            "aarch64-unknown-linux-gnu"
        );
    }

    #[test]
    fn known_hosts_cover_linux_arm_and_riscv() {
        for h in [
            "aarch64-unknown-linux-gnu",
            "aarch64-unknown-linux-musl",
            "riscv64-unknown-linux-gnu",
        ] {
            assert!(KNOWN_HOSTS.contains(&h), "{h} missing from KNOWN_HOSTS");
        }
    }

    #[test]
    fn arch_aliases_normalise() {
        assert_eq!(server_arch("arm64").unwrap(), "aarch64");
        assert_eq!(server_arch("amd64").unwrap(), "x86_64");
    }

    #[test]
    fn unknown_os_and_arch_error() {
        assert_eq!(
            host_triple_from("x86_64", "plan9"),
            Err(TripleError::UnsupportedOs("plan9".into()))
        );
        assert_eq!(
            host_triple_from("sparc", "linux"),
            Err(TripleError::UnsupportedArch("sparc".into()))
        );
    }

    #[test]
    fn linux_x86_64_resolves_directly() {
        // libc-aware: gnu on a glibc runner, musl on Alpine — assert the shape,
        // not the exact flavour, so the test is independent of the build host.
        let r = resolve_host("x86_64", "linux").unwrap().unwrap();
        assert!(r.native.starts_with("x86_64-unknown-linux-"));
        assert!(!r.via_emulation);
    }

    #[test]
    fn mac_x64_is_a_native_host() {
        // Intel Macs have their own x86_64 host toolchain — NOT an arm fallback
        // (Intel can't run arm64; Rosetta only goes the other way).
        let r = resolve_host("x86_64", "macos").unwrap().unwrap();
        assert_eq!(r.native, "x86_64-apple-macosx");
        assert_eq!(r.host_archive, "x86_64-apple-macosx");
        assert!(!r.via_emulation);
    }

    #[test]
    fn win_arm64_falls_back_to_x64_host() {
        let r = resolve_host("aarch64", "windows").unwrap().unwrap();
        assert_eq!(r.host_archive, "x86_64-pc-windows-msvc");
        assert!(r.via_emulation);
    }

    #[test]
    fn linux_arm64_now_has_a_host() {
        // Previously excluded; the SDK ships aarch64 Linux host toolchains, so this
        // must resolve to a native host (gnu or musl) rather than None.
        let r = resolve_host("aarch64", "linux").unwrap().unwrap();
        assert!(r.native.starts_with("aarch64-unknown-linux-"));
        assert!(!r.via_emulation);
    }
}
