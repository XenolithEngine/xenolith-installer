//! Fetch and unpack the engine source bundle — the `STAPPLER_ROOT` that holds
//! the build system, modules and the `toolchains/` directory where host/target
//! toolchains are installed.
//!
//! It is served as a submodule-complete `.tar.gz` from a GitHub release
//! (temporary, until a self-hosted forge serves engine archives), so the end
//! user needs no git. Integrity is checked against the release `SHA256SUMS.txt`.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use flate2::read::GzDecoder;

use crate::dirs::Layout;
use crate::hash::sha256_hex;

/// Release that hosts the engine bundles (this installer repo, `engine-snapshot`).
const RELEASE_BASE: &str =
    "https://github.com/XenolithEngine/xenolith-installer/releases/download/engine-snapshot";

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("download failed: {0}")]
    Http(String),
    #[error("no checksum published for {0}")]
    ChecksumMissing(String),
    #[error("checksum mismatch for {file}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        file: String,
        expected: String,
        actual: String,
    },
    #[error("unpack failed: {0}")]
    Unpack(String),
    #[error("filesystem: {0}")]
    Io(#[from] std::io::Error),
}

/// A named engine bundle, identified by the git ref it was built from.
pub struct EngineBundle {
    pub reference: String,
}

/// Recorded after a successful install — surfaced as the engine/runtime version.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EngineInfo {
    pub reference: String,
    pub sha256: String,
}

impl EngineInfo {
    /// Short, human-facing revision (first 8 hex of the bundle hash).
    pub fn short(&self) -> String {
        self.sha256.chars().take(8).collect()
    }
}

impl EngineBundle {
    pub fn new(reference: impl Into<String>) -> Self {
        Self {
            reference: reference.into(),
        }
    }

    fn archive_name(&self) -> String {
        format!("xenolith-engine-{}.tar.gz", self.reference)
    }
    fn archive_url(&self) -> String {
        format!("{RELEASE_BASE}/{}", self.archive_name())
    }
    fn checksums_url(&self) -> String {
        format!("{RELEASE_BASE}/SHA256SUMS.txt")
    }

    /// Download the bundle, verify it against `SHA256SUMS.txt`, and unpack it so
    /// `layout.engine_dir("master")` directly contains the engine tree. `progress`
    /// reports cumulative downloaded bytes.
    pub fn install(
        &self,
        layout: &Layout,
        progress: &mut dyn FnMut(u64),
    ) -> Result<EngineInfo, EngineError> {
        let sums = parse_checksums(&http_get_text(&self.checksums_url())?);
        let expected = sums
            .get(&self.archive_name())
            .ok_or_else(|| EngineError::ChecksumMissing(self.archive_name()))?
            .clone();

        let bytes = http_get_bytes(&self.archive_url(), progress)?;
        let actual = sha256_hex(&bytes);
        if actual != expected {
            return Err(EngineError::ChecksumMismatch {
                file: self.archive_name(),
                expected,
                actual,
            });
        }

        let wrapper = format!("xenolith-engine-{}", self.reference);
        unpack_into(&bytes, &layout.engine_dir(&self.reference), &wrapper)?;
        Ok(EngineInfo {
            reference: self.reference.clone(),
            sha256: actual,
        })
    }
}

/// Parse a `sha256  filename` listing (the `*name` binary marker is tolerated).
fn parse_checksums(txt: &str) -> HashMap<String, String> {
    txt.lines()
        .filter_map(|l| {
            let mut it = l.split_whitespace();
            let sha = it.next()?;
            let name = it.next()?.trim_start_matches('*');
            Some((name.to_string(), sha.to_string()))
        })
        .collect()
}

fn http_get_text(url: &str) -> Result<String, EngineError> {
    ureq::get(url)
        .call()
        .map_err(|e| EngineError::Http(e.to_string()))?
        .into_string()
        .map_err(|e| EngineError::Http(e.to_string()))
}

fn http_get_bytes(url: &str, progress: &mut dyn FnMut(u64)) -> Result<Vec<u8>, EngineError> {
    let mut reader = ureq::get(url)
        .call()
        .map_err(|e| EngineError::Http(e.to_string()))?
        .into_reader();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut chunk)
            .map_err(|e| EngineError::Http(e.to_string()))?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        progress(buf.len() as u64);
    }
    Ok(buf)
}

/// gunzip + untar `bytes` and place the tree at `dest`, promoting the single
/// `xenolith-engine-<ref>/` wrapper so `dest` directly holds `make/`, `stappler/`,
/// `toolchains/`, … The swap into place is atomic (staging is a sibling of dest).
fn unpack_into(bytes: &[u8], dest: &Path, wrapper: &str) -> Result<(), EngineError> {
    let parent = dest
        .parent()
        .expect("engine_dir always has a parent")
        .to_path_buf();
    std::fs::create_dir_all(&parent)?;
    let staging = parent.join(".engine-staging");
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;

    let mut archive = tar::Archive::new(GzDecoder::new(bytes));
    archive
        .unpack(&staging)
        .map_err(|e| EngineError::Unpack(e.to_string()))?;

    let wrapped = staging.join(wrapper);
    let root = if wrapped.is_dir() {
        wrapped
    } else {
        staging.clone()
    };
    if dest.exists() {
        std::fs::remove_dir_all(dest)?;
    }
    std::fs::rename(&root, dest)?;
    if root != staging && staging.exists() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn targz(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(gz);
        for (path, data) in entries {
            let mut h = tar::Header::new_gnu();
            h.set_size(data.len() as u64);
            h.set_mode(0o644);
            h.set_cksum();
            tar.append_data(&mut h, path, &data[..]).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn parses_sha256sums() {
        let txt = "abc123  xenolith-engine-master.tar.gz\ndef456 *xenolith-engine-master.zip\n";
        let m = parse_checksums(txt);
        assert_eq!(m["xenolith-engine-master.tar.gz"], "abc123");
        assert_eq!(m["xenolith-engine-master.zip"], "def456");
    }

    #[test]
    fn unpack_promotes_the_engine_wrapper_dir() {
        let bytes = targz(&[
            ("xenolith-engine-master/make/universal.mk", b"include ..."),
            ("xenolith-engine-master/toolchains/Makefile", b"host:"),
        ]);
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        unpack_into(
            &bytes,
            &layout.engine_dir("master"),
            "xenolith-engine-master",
        )
        .unwrap();
        // engine_dir directly holds the build system, no extra nesting.
        assert!(layout
            .engine_dir("master")
            .join("make/universal.mk")
            .exists());
        assert!(layout
            .engine_dir("master")
            .join("toolchains/Makefile")
            .exists());
        assert!(!layout
            .engine_dir("master")
            .join("xenolith-engine-master")
            .exists());
    }

    #[test]
    fn unpack_replaces_previous_engine() {
        let home = tempfile::tempdir().unwrap();
        let layout = Layout::from_home(home.path());
        let dir = layout.engine_dir("master");
        std::fs::create_dir_all(dir.join("stale")).unwrap();
        std::fs::File::create(dir.join("stale/old.txt")).unwrap();

        let bytes = targz(&[("xenolith-engine-master/make/universal.mk", b"x")]);
        unpack_into(&bytes, &dir, "xenolith-engine-master").unwrap();
        assert!(!dir.join("stale").exists());
        assert!(dir.join("make/universal.mk").exists());
    }
}
