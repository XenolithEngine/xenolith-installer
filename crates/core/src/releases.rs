//! Discover the SDK releases published under `/releases` and pick the newest.
//!
//! The server hosts one directory per release, e.g. `sdk-v0alpha0`,
//! `sdk-v0beta0`. The installer must NOT hard-code a single release name —
//! when a newer pre-release (beta after alpha) is published it has to surface
//! automatically. Names are parsed into `(major, stage, stage number)` and
//! ordered so the newest sorts last; an unrecognised directory is ignored
//! rather than mis-ranked.

use crate::manifest::RemoteEntry;
use crate::transport::{list_with_retries, Transport, TransportError};

/// Root directory that holds one sub-directory per release.
pub const RELEASES_ROOT: &str = "/releases";
/// Release directory names start with this (`sdk-v0alpha0` → `0alpha0`).
const PREFIX: &str = "sdk-v";

/// Pre-release stage, ordered oldest → newest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Alpha,
    Beta,
    Rc,
    Stable,
}

impl Stage {
    fn parse(word: &str) -> Option<Stage> {
        match word {
            "alpha" => Some(Stage::Alpha),
            "beta" => Some(Stage::Beta),
            "rc" => Some(Stage::Rc),
            "" => Some(Stage::Stable),
            _ => None,
        }
    }
}

/// A parsed release directory, ordered oldest → newest by
/// `(major, stage, stage_num)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Full directory name as published, e.g. `sdk-v0beta0`.
    pub name: String,
    pub major: u32,
    pub stage: Stage,
    pub stage_num: u32,
}

impl Release {
    /// Remote path of this release directory, e.g. `/releases/sdk-v0beta0`.
    pub fn base(&self, root: &str) -> String {
        format!("{root}/{}", self.name)
    }

    fn key(&self) -> (u32, Stage, u32) {
        (self.major, self.stage, self.stage_num)
    }
}

impl PartialOrd for Release {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Release {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

/// Parse a release directory name. Returns `None` for anything that is not a
/// recognised `sdk-v<major><stage><num>` name (so foreign dirs are skipped).
pub fn parse_release(name: &str) -> Option<Release> {
    let rest = name.strip_prefix(PREFIX)?;

    let digits = |s: &str| s.chars().take_while(|c| c.is_ascii_digit()).count();

    // Leading digits → major version (required).
    let mlen = digits(rest);
    if mlen == 0 {
        return None;
    }
    let major: u32 = rest[..mlen].parse().ok()?;
    let rest = &rest[mlen..];

    // Alphabetic run → stage word ("" = stable release).
    let slen = rest.chars().take_while(|c| c.is_ascii_alphabetic()).count();
    let stage = Stage::parse(&rest[..slen])?;
    let rest = &rest[slen..];

    // Trailing digits → stage number (absent = 0).
    let stage_num: u32 = if rest.is_empty() {
        0
    } else if digits(rest) == rest.len() {
        rest.parse().ok()?
    } else {
        return None; // trailing junk → not a release name
    };

    Some(Release {
        name: name.to_string(),
        major,
        stage,
        stage_num,
    })
}

/// Pick the newest parseable release from raw directory entries (non-dirs and
/// unrecognised names are ignored).
pub fn newest_from_entries(entries: &[RemoteEntry]) -> Option<Release> {
    entries
        .iter()
        .filter(|e| e.is_dir)
        .filter_map(|e| parse_release(&e.name))
        .max()
}

/// List `root` over the transport and return the newest published release.
/// `Ok(None)` means the directory listed but held no recognisable release.
pub fn latest_release(
    transport: &dyn Transport,
    root: &str,
    attempts: u32,
) -> Result<Option<Release>, TransportError> {
    let entries = list_with_retries(transport, &format!("{root}/"), attempts)?;
    Ok(newest_from_entries(&entries))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::testing::MockTransport;

    #[test]
    fn parses_alpha_and_beta() {
        let a = parse_release("sdk-v0alpha0").unwrap();
        assert_eq!((a.major, a.stage, a.stage_num), (0, Stage::Alpha, 0));
        let b = parse_release("sdk-v0beta0").unwrap();
        assert_eq!((b.major, b.stage, b.stage_num), (0, Stage::Beta, 0));
    }

    #[test]
    fn parses_stable_and_rc_and_numbers() {
        assert_eq!(parse_release("sdk-v1").unwrap().stage, Stage::Stable);
        let rc = parse_release("sdk-v2rc3").unwrap();
        assert_eq!((rc.major, rc.stage, rc.stage_num), (2, Stage::Rc, 3));
    }

    #[test]
    fn rejects_foreign_names() {
        assert!(parse_release("infrastructure").is_none());
        assert!(parse_release("v0").is_none());
        assert!(parse_release("sdk-v").is_none());
        assert!(parse_release("sdk-valpha0").is_none()); // no major
        assert!(parse_release("sdk-v0gamma0").is_none()); // unknown stage
    }

    #[test]
    fn beta_outranks_alpha_same_major() {
        let a = parse_release("sdk-v0alpha0").unwrap();
        let b = parse_release("sdk-v0beta0").unwrap();
        assert!(b > a);
    }

    #[test]
    fn ordering_major_then_stage_then_num() {
        // major dominates stage: v1alpha beats v0beta.
        assert!(parse_release("sdk-v1alpha0").unwrap() > parse_release("sdk-v0beta0").unwrap());
        // stable beats rc; rc beats beta.
        assert!(parse_release("sdk-v1").unwrap() > parse_release("sdk-v1rc9").unwrap());
        assert!(parse_release("sdk-v1rc0").unwrap() > parse_release("sdk-v1beta9").unwrap());
        // stage number breaks ties within a stage.
        assert!(parse_release("sdk-v0beta2").unwrap() > parse_release("sdk-v0beta1").unwrap());
    }

    #[test]
    fn newest_picks_beta_over_alpha_ignoring_files() {
        let entries = vec![
            RemoteEntry {
                name: "infrastructure".into(),
                size: 0,
                is_dir: true,
            },
            RemoteEntry {
                name: "sdk-v0alpha0".into(),
                size: 0,
                is_dir: true,
            },
            RemoteEntry {
                name: "sdk-v0beta0".into(),
                size: 0,
                is_dir: true,
            },
            RemoteEntry {
                name: "README".into(),
                size: 12,
                is_dir: false,
            },
        ];
        let r = newest_from_entries(&entries).unwrap();
        assert_eq!(r.name, "sdk-v0beta0");
        assert_eq!(r.base(RELEASES_ROOT), "/releases/sdk-v0beta0");
    }

    #[test]
    fn newest_is_none_when_no_release_dirs() {
        let entries = vec![RemoteEntry {
            name: "infrastructure".into(),
            size: 0,
            is_dir: true,
        }];
        assert!(newest_from_entries(&entries).is_none());
    }

    #[test]
    fn latest_release_lists_over_transport() {
        let t = MockTransport::new().with_listing(
            "/releases/",
            "drwxrwxr-x 2 0 0 4096 Jun 09 00:00 sdk-v0alpha0\n\
             drwxrwxr-x 2 0 0 4096 Jun 26 00:00 sdk-v0beta0",
        );
        let r = latest_release(&t, RELEASES_ROOT, 3).unwrap().unwrap();
        assert_eq!(r.name, "sdk-v0beta0");
    }
}
