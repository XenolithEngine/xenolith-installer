//! Remote catalogue of installable components.
//!
//! Two ways to obtain it (see [`crate::transport`]):
//!   * **preferred** — a server-side `manifest.json`, once one is published,
//!   * **fallback (v0)** — parse the FTP `LIST` output of `hosts/` and
//!     `targets/`. vsFTPd's passive listing is flaky, so the transport retries;
//!     this module only does the *parsing*, which is pure and unit-tested
//!     against real captured output.
//!
//! Security rule: a `.tar.xz` with no matching `.tar.xz.sig` is DROPPED, never
//! offered. We never present an unsigned artifact for install.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Build host toolchain (`/releases/<rel>/hosts/`).
    Host,
    /// Cross-compilation target sysroot (`/releases/<rel>/targets/`).
    Target,
}

impl Kind {
    /// The directory segment this kind lives under, both on the FTP and in the
    /// local SDK layout.
    pub fn dir(self) -> &'static str {
        match self {
            Kind::Host => "hosts",
            Kind::Target => "targets",
        }
    }
}

/// One line of an FTP `LIST` response, minimally parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub size: u64,
    pub is_dir: bool,
}

/// An installable component: a signed `.tar.xz` archive for one triple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Component {
    /// Full id incl. any variant, e.g. `aarch64-apple-macosx+sprt`.
    pub id: String,
    /// Triple without the `+variant` suffix, e.g. `aarch64-apple-macosx`.
    pub triple: String,
    /// Optional variant after `+`, e.g. `Some("sprt")`.
    pub variant: Option<String>,
    pub kind: Kind,
    /// Archive file name within its directory.
    pub archive: String,
    /// Detached-signature file name (always present — unsigned are dropped).
    pub sig: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub release: String,
    pub components: Vec<Component>,
}

const ARCHIVE_EXT: &str = ".tar.xz";
const SIG_EXT: &str = ".tar.xz.sig";

/// Parse vsFTPd `LIST` (Unix `ls -l`-style) output into entries.
///
/// Example line:
/// `-rw-r--r--    1 1000     1000     46516100 Jun 08 19:39 name.tar.xz`
pub fn parse_ftp_list(text: &str) -> Vec<RemoteEntry> {
    text.lines().filter_map(parse_ftp_line).collect()
}

fn parse_ftp_line(line: &str) -> Option<RemoteEntry> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }
    let perms = line.split_whitespace().next()?;
    let is_dir = perms.starts_with('d');
    if perms.starts_with('l') {
        // Symlinks aren't part of the catalogue; ignore (the `->` also breaks
        // the column count).
        return None;
    }
    // Columns: perms links owner group size mon day time/year name...
    // Column gaps are runs of spaces, so tokenise with `split_whitespace` and
    // rejoin everything past the 8th field as the name (filenames here use at
    // most single spaces, which a single-space join preserves).
    let fields: Vec<&str> = line.split_whitespace().collect();
    if fields.len() < 9 {
        return None;
    }
    let size: u64 = fields[4].parse().ok()?;
    let name = fields[8..].join(" ");
    if name.is_empty() || name == "." || name == ".." {
        return None;
    }
    Some(RemoteEntry { name, size, is_dir })
}

fn split_variant(triple_with_variant: &str) -> (String, Option<String>) {
    match triple_with_variant.split_once('+') {
        Some((base, var)) => (base.to_string(), Some(var.to_string())),
        None => (triple_with_variant.to_string(), None),
    }
}

/// Build components from one directory's entries. Pairs each `*.tar.xz` with its
/// `*.tar.xz.sig`; archives lacking a signature are dropped (and returned so the
/// caller can `log()` what was skipped — no silent truncation).
pub fn components_from_entries(
    entries: &[RemoteEntry],
    kind: Kind,
) -> (Vec<Component>, Vec<String>) {
    use std::collections::HashSet;
    let sigs: HashSet<&str> = entries
        .iter()
        .filter(|e| !e.is_dir && e.name.ends_with(SIG_EXT))
        .map(|e| e.name.as_str())
        .collect();

    let mut components = Vec::new();
    let mut dropped = Vec::new();
    for e in entries.iter().filter(|e| !e.is_dir) {
        if !e.name.ends_with(ARCHIVE_EXT) || e.name.ends_with(SIG_EXT) {
            continue;
        }
        let sig = format!("{}.sig", e.name);
        if !sigs.contains(sig.as_str()) {
            dropped.push(e.name.clone());
            continue;
        }
        let id = e.name.trim_end_matches(ARCHIVE_EXT).to_string();
        let (triple, variant) = split_variant(&id);
        components.push(Component {
            id,
            triple,
            variant,
            kind,
            archive: e.name.clone(),
            sig,
            size: e.size,
        });
    }
    (components, dropped)
}

impl Manifest {
    /// Assemble a manifest from the raw `LIST` text of the `hosts/` and
    /// `targets/` directories.
    pub fn from_dir_listings(
        release: impl Into<String>,
        hosts_list: &str,
        targets_list: &str,
    ) -> (Self, Vec<String>) {
        let (mut components, mut dropped) =
            components_from_entries(&parse_ftp_list(hosts_list), Kind::Host);
        let (targets, t_dropped) =
            components_from_entries(&parse_ftp_list(targets_list), Kind::Target);
        components.extend(targets);
        dropped.extend(t_dropped);
        (
            Manifest {
                release: release.into(),
                components,
            },
            dropped,
        )
    }

    pub fn hosts(&self) -> impl Iterator<Item = &Component> {
        self.components.iter().filter(|c| c.kind == Kind::Host)
    }

    pub fn targets(&self) -> impl Iterator<Item = &Component> {
        self.components.iter().filter(|c| c.kind == Kind::Target)
    }

    pub fn find(&self, id: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.id == id)
    }

    /// Find a component by id AND kind. Use this to disambiguate a triple that
    /// exists as both a host and a target (e.g. `aarch64-apple-macosx`).
    pub fn find_kind(&self, id: &str, kind: Kind) -> Option<&Component> {
        self.components
            .iter()
            .find(|c| c.id == id && c.kind == kind)
    }
}

/// Fetch and assemble a release manifest over a [`Transport`], retrying the
/// flaky directory listings. Returns the manifest plus the names of any unsigned
/// artifacts that were skipped. Shared by the CLI and GUI front-ends.
pub fn fetch_manifest(
    transport: &dyn crate::transport::Transport,
    remote_base: &str,
    release: &str,
    attempts: u32,
) -> Result<(Manifest, Vec<String>), crate::transport::TransportError> {
    use crate::transport::list_with_retries;
    let hosts = list_with_retries(transport, &format!("{remote_base}/hosts/"), attempts)?;
    let targets = list_with_retries(transport, &format!("{remote_base}/targets/"), attempts)?;
    let (mut components, mut dropped) = components_from_entries(&hosts, Kind::Host);
    let (t, td) = components_from_entries(&targets, Kind::Target);
    components.extend(t);
    dropped.extend(td);
    Ok((
        Manifest {
            release: release.to_string(),
            components,
        },
        dropped,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real captured output from ftp://stappler.dev/releases/sdk-v0alpha0/hosts/
    const HOSTS_FIXTURE: &str = "\
-rw-r--r--    1 1000     1000     46516100 Jun 08 19:39 aarch64-apple-macosx.tar.xz
-rw-r--r--    1 1000     1000          566 Jun 08 19:40 aarch64-apple-macosx.tar.xz.sig
-rw-r--r--    1 1000     1000     76889340 Jun 08 19:39 x86_64-pc-windows-msvc.tar.xz
-rw-r--r--    1 1000     1000          566 Jun 08 19:40 x86_64-pc-windows-msvc.tar.xz.sig
-rw-r--r--    1 1000     1000     76321624 Jun 08 19:39 x86_64-unknown-linux-gnu.tar.xz
-rw-r--r--    1 1000     1000          566 Jun 08 19:40 x86_64-unknown-linux-gnu.tar.xz.sig";

    // Real captured output from .../targets/ (trimmed to a representative subset
    // incl. a `+sprt` variant and the android NDK).
    const TARGETS_FIXTURE: &str = "\
-rw-r--r--    1 1000     1000     21112904 Jun 08 19:41 aarch64-apple-macosx+sprt.tar.xz
-rw-r--r--    1 1000     1000          566 Jun 08 19:42 aarch64-apple-macosx+sprt.tar.xz.sig
-rw-r--r--    1 1000     1000     20363396 Jun 08 19:41 aarch64-apple-macosx.tar.xz
-rw-r--r--    1 1000     1000          566 Jun 08 19:42 aarch64-apple-macosx.tar.xz.sig
-rw-r--r--    1 1000     1000     13123808 Jun 08 19:41 aarch64-unknown-linux-android.tar.xz
-rw-r--r--    1 1000     1000          566 Jun 08 19:42 aarch64-unknown-linux-android.tar.xz.sig";

    #[test]
    fn parses_a_plain_file_line() {
        let e = parse_ftp_line(
            "-rw-r--r--    1 1000     1000     46516100 Jun 08 19:39 aarch64-apple-macosx.tar.xz",
        )
        .unwrap();
        assert_eq!(e.name, "aarch64-apple-macosx.tar.xz");
        assert_eq!(e.size, 46_516_100);
        assert!(!e.is_dir);
    }

    #[test]
    fn parses_a_directory_line() {
        let e = parse_ftp_line("drwxrwxr-x    2 1000     1000         4096 Jun 08 19:52 hosts")
            .unwrap();
        assert_eq!(e.name, "hosts");
        assert!(e.is_dir);
    }

    #[test]
    fn skips_blank_dotdirs_and_symlinks() {
        assert_eq!(parse_ftp_line(""), None);
        assert_eq!(parse_ftp_line("   "), None);
        assert_eq!(
            parse_ftp_line("lrwxrwxrwx 1 0 0 7 Jan 1 2020 latest -> v0alpha0"),
            None
        );
    }

    #[test]
    fn full_hosts_listing_yields_three_signed_components() {
        let entries = parse_ftp_list(HOSTS_FIXTURE);
        // 3 archives + 3 sigs = 6 entries.
        assert_eq!(entries.len(), 6);
        let (comps, dropped) = components_from_entries(&entries, Kind::Host);
        assert_eq!(comps.len(), 3);
        assert!(dropped.is_empty());
        assert!(comps.iter().all(|c| c.kind == Kind::Host));
        let linux = comps.iter().find(|c| c.triple.contains("linux")).unwrap();
        assert_eq!(linux.id, "x86_64-unknown-linux-gnu");
        assert_eq!(linux.archive, "x86_64-unknown-linux-gnu.tar.xz");
        assert_eq!(linux.sig, "x86_64-unknown-linux-gnu.tar.xz.sig");
        assert_eq!(linux.size, 76_321_624);
        assert_eq!(linux.variant, None);
    }

    #[test]
    fn variant_suffix_is_split_out() {
        let (comps, _) = components_from_entries(&parse_ftp_list(TARGETS_FIXTURE), Kind::Target);
        let sprt = comps.iter().find(|c| c.variant.is_some()).unwrap();
        assert_eq!(sprt.id, "aarch64-apple-macosx+sprt");
        assert_eq!(sprt.triple, "aarch64-apple-macosx");
        assert_eq!(sprt.variant.as_deref(), Some("sprt"));
    }

    #[test]
    fn unsigned_archive_is_dropped_not_offered() {
        let listing = "\
-rw-r--r-- 1 0 0 100 Jun 08 19:39 signed.tar.xz
-rw-r--r-- 1 0 0  10 Jun 08 19:39 signed.tar.xz.sig
-rw-r--r-- 1 0 0 200 Jun 08 19:39 unsigned.tar.xz";
        let (comps, dropped) = components_from_entries(&parse_ftp_list(listing), Kind::Target);
        assert_eq!(comps.len(), 1);
        assert_eq!(comps[0].id, "signed");
        assert_eq!(dropped, vec!["unsigned.tar.xz".to_string()]);
    }

    #[test]
    fn manifest_merges_hosts_and_targets() {
        let (m, dropped) =
            Manifest::from_dir_listings("sdk-v0alpha0", HOSTS_FIXTURE, TARGETS_FIXTURE);
        assert_eq!(m.release, "sdk-v0alpha0");
        assert_eq!(m.hosts().count(), 3);
        assert_eq!(m.targets().count(), 3);
        assert!(dropped.is_empty());
        assert_eq!(m.find("x86_64-pc-windows-msvc").unwrap().kind, Kind::Host);
    }

    #[test]
    fn find_kind_disambiguates_a_shared_triple() {
        // aarch64-apple-macosx exists as BOTH a host and a target.
        let hosts = "\
-rw-r--r-- 1 0 0 100 Jun 08 19:39 aarch64-apple-macosx.tar.xz
-rw-r--r-- 1 0 0   1 Jun 08 19:40 aarch64-apple-macosx.tar.xz.sig";
        let targets = "\
-rw-r--r-- 1 0 0 200 Jun 08 19:39 aarch64-apple-macosx.tar.xz
-rw-r--r-- 1 0 0   1 Jun 08 19:40 aarch64-apple-macosx.tar.xz.sig";
        let (m, _) = Manifest::from_dir_listings("rel", hosts, targets);
        let host = m.find_kind("aarch64-apple-macosx", Kind::Host).unwrap();
        let target = m.find_kind("aarch64-apple-macosx", Kind::Target).unwrap();
        assert_eq!(host.kind, Kind::Host);
        assert_eq!(host.size, 100);
        assert_eq!(target.kind, Kind::Target);
        assert_eq!(target.size, 200);
        // Plain `find` returns the first (host) — which is exactly the ambiguity
        // `find_kind` exists to resolve.
        assert_eq!(m.find("aarch64-apple-macosx").unwrap().kind, Kind::Host);
    }

    #[test]
    fn manifest_round_trips_through_json() {
        let (m, _) = Manifest::from_dir_listings("rel", HOSTS_FIXTURE, TARGETS_FIXTURE);
        let json = serde_json::to_string(&m).unwrap();
        let back: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(m, back);
    }
}
