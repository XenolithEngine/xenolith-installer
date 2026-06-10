//! Archive extraction: `.tar.xz` → directory tree.
//!
//! Unpacks with permissions and symlinks preserved (Unix exec bits survive).
//! Callers extract into a scratch directory and then atomically rename it into
//! place, so an interrupted extract never leaves a half-populated install.

use std::io::Read;
use std::path::Path;

use tar::Archive;
use xz2::read::XzDecoder;

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("decompress/unpack error: {0}")]
    Io(#[from] std::io::Error),
}

/// Extract a `.tar.xz` stream into `dest` (created if missing).
pub fn extract_tar_xz<R: Read>(reader: R, dest: &Path) -> Result<(), ExtractError> {
    std::fs::create_dir_all(dest)?;
    let mut archive = Archive::new(XzDecoder::new(reader));
    archive.set_preserve_permissions(true);
    archive.set_overwrite(true);
    archive.unpack(dest)?;
    Ok(())
}

/// Convenience over in-memory bytes.
pub fn extract_tar_xz_bytes(bytes: &[u8], dest: &Path) -> Result<(), ExtractError> {
    extract_tar_xz(bytes, dest)
}

pub mod testing {
    //! Build a `.tar.xz` in memory so tests (here and in dependent crates)
    //! don't need a fixture file on disk.
    use std::io::Write;

    use xz2::write::XzEncoder;

    /// `(path, contents, executable)`
    pub fn make_tar_xz(entries: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut tar_bytes = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut tar_bytes);
            for (path, data, exec) in entries {
                let mut header = tar::Header::new_gnu();
                header.set_size(data.len() as u64);
                header.set_mode(if *exec { 0o755 } else { 0o644 });
                header.set_cksum();
                builder.append_data(&mut header, path, *data).unwrap();
            }
            builder.finish().unwrap();
        }
        let mut enc = XzEncoder::new(Vec::new(), 6);
        enc.write_all(&tar_bytes).unwrap();
        enc.finish().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::testing::make_tar_xz;
    use super::*;

    #[test]
    fn extracts_files_and_nested_dirs_with_content() {
        let archive = make_tar_xz(&[
            ("bin/xenolith", b"ELF...", true),
            ("lib/libfoo.a", b"archive-bytes", false),
            ("readme.txt", b"hello", false),
        ]);
        let dir = tempfile::tempdir().unwrap();
        extract_tar_xz_bytes(&archive, dir.path()).unwrap();

        assert_eq!(
            std::fs::read(dir.path().join("readme.txt")).unwrap(),
            b"hello"
        );
        assert_eq!(
            std::fs::read(dir.path().join("lib/libfoo.a")).unwrap(),
            b"archive-bytes"
        );
        assert!(dir.path().join("bin/xenolith").exists());
    }

    #[cfg(unix)]
    #[test]
    fn preserves_unix_exec_bit() {
        use std::os::unix::fs::PermissionsExt;
        let archive = make_tar_xz(&[("bin/tool", b"#!/bin/sh\n", true)]);
        let dir = tempfile::tempdir().unwrap();
        extract_tar_xz_bytes(&archive, dir.path()).unwrap();
        let mode = std::fs::metadata(dir.path().join("bin/tool"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "executable bit should survive extraction");
    }

    #[test]
    fn creates_destination_when_missing() {
        let archive = make_tar_xz(&[("f", b"x", false)]);
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a/b/c");
        extract_tar_xz_bytes(&archive, &dest).unwrap();
        assert!(dest.join("f").exists());
    }
}
