//! Target-triple detection and mapping.
//!
//! The Xenolith SDK on the FTP names every artifact with an LLVM/Rust-style
//! target triple: `<arch>-<vendor>-<os>[-<abi>][+<variant>]`. We must map the
//! current platform (and, crucially, its *native* arch under emulation) to the
//! server's naming, which differs from Rust's in one place: macOS is
//! `apple-macosx`, NOT `apple-darwin`.

use std::fmt;

/// Host triples for which a host toolchain archive actually exists on the FTP
/// (`/releases/<rel>/hosts/`). There is intentionally no `linux-arm64`,
/// `win-arm64` or `macos-x64` host yet — see [`host_fallback`].
pub const KNOWN_HOSTS: [&str; 4] = [
    "aarch64-apple-macosx",
    "x86_64-apple-macosx",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TripleError {
    #[error("unsupported OS for SDK host: {0}")]
    UnsupportedOs(String),
    #[error("unsupported architecture for SDK host: {0}")]
    UnsupportedArch(String),
}

/// Map a Rust `std::env::consts::OS` value to the server's vendor-os segment.
pub fn server_os(os: &str) -> Result<&'static str, TripleError> {
    Ok(match os {
        "macos" | "ios" => "apple-macosx",
        "windows" => "pc-windows-msvc",
        "linux" | "android" => "unknown-linux-gnu",
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

/// Build the server triple string for a given (arch, os) pair.
///
/// This is the pure core of detection — it does NOT look at the running
/// process; callers pass in the *native* arch/os (see [`detect_host_triple`]).
pub fn host_triple_from(arch: &str, os: &str) -> Result<String, TripleError> {
    Ok(format!("{}-{}", server_arch(arch)?, server_os(os)?))
}

/// When no host toolchain exists for `triple`, pick the host the current
/// machine can run via emulation (mac-x64 → arm host under Rosetta, win-arm64 →
/// x64 host under WOW64). Returns `None` when nothing can run it (e.g. linux-arm64).
pub fn host_fallback(triple: &str) -> Option<&'static str> {
    if KNOWN_HOSTS.contains(&triple) {
        // Caller should normally check this first, but be forgiving.
        return KNOWN_HOSTS.into_iter().find(|h| *h == triple);
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
    let native = host_triple_from(arch, os)?;
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
        assert_eq!(server_os("ios").unwrap(), "apple-macosx");
        assert_eq!(server_os("android").unwrap(), "unknown-linux-gnu");
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
    fn all_three_known_hosts_resolve_directly() {
        for h in KNOWN_HOSTS {
            assert!(KNOWN_HOSTS.contains(&h));
        }
        let r = resolve_host("x86_64", "linux").unwrap().unwrap();
        assert_eq!(r.host_archive, "x86_64-unknown-linux-gnu");
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
    fn linux_arm64_has_no_host() {
        // Valid triple, but no host can run it → None (not an error).
        assert_eq!(resolve_host("aarch64", "linux").unwrap(), None);
    }
}
