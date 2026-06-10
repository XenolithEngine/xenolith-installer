//! Network transport abstraction.
//!
//! The real implementation (FTP via a blocking client, later HTTPS) lives
//! behind this trait so the rest of core — manifest building, download,
//! install — is testable without a network and swappable once an HTTPS mirror
//! exists. The interface is synchronous; callers run it on a worker thread and
//! the GUI marshals progress back to the UI.

use std::io::Write;

use crate::manifest::{parse_ftp_list, RemoteEntry};

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("transport i/o: {0}")]
    Io(String),
}

pub trait Transport {
    /// List a directory, returning parsed entries.
    fn list(&self, path: &str) -> Result<Vec<RemoteEntry>, TransportError>;

    /// Stream a file into `sink`, reporting cumulative bytes through `progress`.
    fn fetch_to(
        &self,
        path: &str,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64),
    ) -> Result<(), TransportError>;

    /// Convenience: fetch a (small) file fully into memory. Built on `fetch_to`.
    fn fetch(&self, path: &str) -> Result<Vec<u8>, TransportError> {
        let mut buf = Vec::new();
        let mut noop = |_| {};
        self.fetch_to(path, &mut buf, &mut noop)?;
        Ok(buf)
    }
}

/// Passive data connections to this server fail *intermittently* — sometimes
/// the first attempt, sometimes none of several. So retry on BOTH the empty-body
/// flap and connection errors; the bounded per-connection timeout keeps retries
/// cheap. A non-empty listing returns at once; if every attempt was empty (never
/// an error) the directory really is empty → `Ok(empty)`; if every attempt
/// errored, surface the last error.
pub fn list_with_retries(
    t: &dyn Transport,
    path: &str,
    attempts: u32,
) -> Result<Vec<RemoteEntry>, TransportError> {
    let mut saw_empty = false;
    let mut last_err = None;
    for _ in 0..attempts.max(1) {
        match t.list(path) {
            Ok(entries) if !entries.is_empty() => return Ok(entries),
            Ok(_) => saw_empty = true,
            Err(e) => last_err = Some(e),
        }
    }
    if saw_empty {
        Ok(Vec::new())
    } else {
        Err(last_err.unwrap_or_else(|| TransportError::Io(format!("no attempts for {path}"))))
    }
}

/// Download `path` fully into memory, retrying the whole transfer on failure.
/// Each attempt uses a fresh buffer (so a partial transfer is never duplicated)
/// and — because the transport drops its connection on error — reconnects. This
/// rides out the server's intermittent passive-data stalls the way FileZilla's
/// transfer retries do.
pub fn fetch_with_retries(
    t: &dyn Transport,
    path: &str,
    attempts: u32,
    progress: &mut dyn FnMut(u64),
) -> Result<Vec<u8>, TransportError> {
    let mut last_err = None;
    for _ in 0..attempts.max(1) {
        let mut buf = Vec::new();
        match t.fetch_to(path, &mut buf, progress) {
            Ok(()) => return Ok(buf),
            Err(e) => last_err = Some(e),
        }
    }
    Err(last_err.unwrap_or_else(|| TransportError::Io(format!("no attempts for {path}"))))
}

/// Test/dev transport backed by in-memory maps. Always compiled so integration
/// tests in dependent crates can use it too.
pub mod testing {
    use std::cell::Cell;
    use std::collections::HashMap;
    use std::io::Write;

    use super::{parse_ftp_list, RemoteEntry, Transport, TransportError};

    #[derive(Default)]
    pub struct MockTransport {
        /// path -> raw FTP LIST text
        listings: HashMap<String, String>,
        /// path -> file bytes
        files: HashMap<String, Vec<u8>>,
        /// number of leading `list` calls that return empty (simulates the flap)
        flaky_empty: Cell<u32>,
        /// number of leading `fetch_to` calls that fail (simulates data stalls)
        flaky_fetch: Cell<u32>,
    }

    impl MockTransport {
        pub fn new() -> Self {
            Self::default()
        }

        pub fn with_listing(mut self, path: &str, list_text: &str) -> Self {
            self.listings
                .insert(path.to_string(), list_text.to_string());
            self
        }

        pub fn with_file(mut self, path: &str, bytes: &[u8]) -> Self {
            self.files.insert(path.to_string(), bytes.to_vec());
            self
        }

        /// Make the next `n` `list` calls return an empty listing (then real).
        pub fn flaky_empty(self, n: u32) -> Self {
            self.flaky_empty.set(n);
            self
        }

        /// Make the next `n` `fetch_to` calls fail (then succeed).
        pub fn flaky_fetch(self, n: u32) -> Self {
            self.flaky_fetch.set(n);
            self
        }
    }

    impl Transport for MockTransport {
        fn list(&self, path: &str) -> Result<Vec<RemoteEntry>, TransportError> {
            let remaining = self.flaky_empty.get();
            if remaining > 0 {
                self.flaky_empty.set(remaining - 1);
                return Ok(Vec::new());
            }
            let text = self
                .listings
                .get(path)
                .ok_or_else(|| TransportError::NotFound(path.to_string()))?;
            Ok(parse_ftp_list(text))
        }

        fn fetch_to(
            &self,
            path: &str,
            sink: &mut dyn Write,
            progress: &mut dyn FnMut(u64),
        ) -> Result<(), TransportError> {
            let remaining = self.flaky_fetch.get();
            if remaining > 0 {
                self.flaky_fetch.set(remaining - 1);
                return Err(TransportError::Io(format!(
                    "simulated data stall for {path}"
                )));
            }
            let bytes = self
                .files
                .get(path)
                .ok_or_else(|| TransportError::NotFound(path.to_string()))?;
            sink.write_all(bytes)
                .map_err(|e| TransportError::Io(e.to_string()))?;
            progress(bytes.len() as u64);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::MockTransport;
    use super::*;

    const LISTING: &str = "-rw-r--r-- 1 0 0 5 Jun 08 19:39 a.tar.xz";

    #[test]
    fn list_parses_via_transport() {
        let t = MockTransport::new().with_listing("hosts/", LISTING);
        let entries = t.list("hosts/").unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].name, "a.tar.xz");
    }

    #[test]
    fn unknown_paths_are_not_found() {
        let t = MockTransport::new();
        assert!(matches!(t.list("nope/"), Err(TransportError::NotFound(_))));
        assert!(matches!(t.fetch("nope"), Err(TransportError::NotFound(_))));
    }

    #[test]
    fn fetch_collects_bytes_and_reports_progress() {
        let t = MockTransport::new().with_file("f", b"hello");
        let mut seen = 0u64;
        let mut sink = Vec::new();
        t.fetch_to("f", &mut sink, &mut |n| seen = n).unwrap();
        assert_eq!(sink, b"hello");
        assert_eq!(seen, 5);
        assert_eq!(t.fetch("f").unwrap(), b"hello");
    }

    #[test]
    fn retries_ride_out_a_flapping_empty_listing() {
        // Two empty responses, then the real one — must succeed within 3 tries.
        let t = MockTransport::new()
            .with_listing("targets/", LISTING)
            .flaky_empty(2);
        let entries = list_with_retries(&t, "targets/", 3).unwrap();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn persistent_empty_is_treated_as_an_empty_directory() {
        let t = MockTransport::new()
            .with_listing("targets/", LISTING)
            .flaky_empty(5);
        // Only 3 attempts against 5 empty flaps → never see content → Ok(empty),
        // since a genuinely empty directory must not look like a failure.
        assert_eq!(list_with_retries(&t, "targets/", 3).unwrap(), Vec::new());
    }

    #[test]
    fn all_attempts_erroring_surfaces_the_error() {
        let t = MockTransport::new(); // no listing registered → NotFound each time
        assert!(list_with_retries(&t, "missing/", 3).is_err());
    }

    #[test]
    fn fetch_retries_past_intermittent_stalls() {
        // Two simulated data stalls, then success — must succeed within 3 tries
        // and return the full bytes exactly once (fresh buffer per attempt).
        let t = MockTransport::new()
            .with_file("f", b"payload")
            .flaky_fetch(2);
        let bytes = fetch_with_retries(&t, "f", 3, &mut |_| {}).unwrap();
        assert_eq!(bytes, b"payload");
    }

    #[test]
    fn fetch_gives_up_after_exhausting_attempts() {
        let t = MockTransport::new()
            .with_file("f", b"payload")
            .flaky_fetch(5);
        assert!(fetch_with_retries(&t, "f", 3, &mut |_| {}).is_err());
    }
}
