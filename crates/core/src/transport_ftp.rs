//! Real anonymous-FTP transport (feature `ftp`).
//!
//! The release server is vsFTPd over plain FTP with anonymous login. The
//! transport mirrors what `curl` and FileZilla do against this server, which is
//! the combination that actually works reliably here:
//!
//!   * **one reused control connection** for all operations, and
//!   * **Extended Passive mode (EPSV)** for the data channel.
//!
//! This was established empirically (`examples/ftp_probe.rs`): plain `PASV`, and
//! opening a fresh connection per operation, both wedge after the first data
//! transfer (the next passive channel hangs to the OS timeout, ~75s). Reused
//! connection + EPSV runs list → list → retr in a couple of seconds.
//!
//! Passive directory listings still flap intermittently, so callers wrap
//! [`Transport::list`] with [`crate::transport::list_with_retries`].

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::Mutex;
use std::time::Duration;

use suppaftp::{
    types::{FileType, Mode},
    FtpError, FtpStream,
};

use crate::manifest::{parse_ftp_list, RemoteEntry};
use crate::transport::{Transport, TransportError};

/// Bound the control connection (connect, login, every command + its response)
/// so a stalled server fails fast instead of hanging on the OS default (~75s+)
/// — then `list_with_retries` / `fetch_with_retries` can re-fetch.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(15);
/// Bound the passive data-connection setup.
const DATA_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
/// Max idle gap mid-transfer before a data read is considered stalled.
const DATA_IO_TIMEOUT: Duration = Duration::from_secs(20);

fn data_connect(addr: SocketAddr) -> Result<TcpStream, FtpError> {
    let s = TcpStream::connect_timeout(&addr, DATA_CONNECT_TIMEOUT)
        .map_err(FtpError::ConnectionError)?;
    let _ = s.set_read_timeout(Some(DATA_IO_TIMEOUT));
    let _ = s.set_write_timeout(Some(DATA_IO_TIMEOUT));
    Ok(s)
}

pub struct FtpTransport {
    host_port: String,
    /// The single reused control connection (lazily opened, dropped on error).
    conn: Mutex<Option<FtpStream>>,
}

impl FtpTransport {
    /// `host_port` like `stappler.dev:21`.
    pub fn new(host_port: impl Into<String>) -> Self {
        FtpTransport {
            host_port: host_port.into(),
            conn: Mutex::new(None),
        }
    }

    fn connect(&self) -> Result<FtpStream, TransportError> {
        // Resolve + connect with a timeout so a dead host fails fast.
        let addr = self
            .host_port
            .to_socket_addrs()
            .map_err(|e| TransportError::Io(e.to_string()))?
            .next()
            .ok_or_else(|| TransportError::Io(format!("cannot resolve {}", self.host_port)))?;
        let mut s = FtpStream::connect_timeout(addr, CONTROL_TIMEOUT)
            .map_err(|e| TransportError::Io(e.to_string()))?
            .passive_stream_builder(data_connect);
        // Bound every control-connection read/write — a stalled LIST or response
        // now errors at CONTROL_TIMEOUT instead of hanging forever.
        let _ = s.get_ref().set_read_timeout(Some(CONTROL_TIMEOUT));
        let _ = s.get_ref().set_write_timeout(Some(CONTROL_TIMEOUT));
        s.login("anonymous", "anonymous@")
            .map_err(|e| TransportError::Io(e.to_string()))?;
        // EPSV: the data port is reused on the control connection's address.
        // Plain PASV wedges this server after the first transfer.
        s.set_mode(Mode::ExtendedPassive);
        let _ = s.transfer_type(FileType::Binary);
        Ok(s)
    }

    /// Run `op` on the reused connection, opening it if needed. On error the
    /// connection is dropped so the next call reconnects from clean state. There
    /// is no in-call retry: a streaming `fetch` must not re-run (it would
    /// duplicate bytes already written to the sink).
    fn with_conn<T>(
        &self,
        op: impl FnOnce(&mut FtpStream) -> Result<T, TransportError>,
    ) -> Result<T, TransportError> {
        let mut guard = self.conn.lock().expect("ftp mutex poisoned");
        if guard.is_none() {
            *guard = Some(self.connect()?);
        }
        let session = guard.as_mut().expect("just ensured Some");
        match op(session) {
            Ok(v) => Ok(v),
            Err(e) => {
                *guard = None;
                Err(e)
            }
        }
    }
}

fn map_err(e: suppaftp::FtpError) -> TransportError {
    let msg = e.to_string();
    if msg.contains("550") {
        TransportError::NotFound(msg)
    } else {
        TransportError::Io(msg)
    }
}

impl Transport for FtpTransport {
    fn list(&self, path: &str) -> Result<Vec<RemoteEntry>, TransportError> {
        self.with_conn(|s| {
            let lines = s.list(Some(path)).map_err(map_err)?;
            Ok(parse_ftp_list(&lines.join("\n")))
        })
    }

    fn fetch_to(
        &self,
        path: &str,
        sink: &mut dyn Write,
        progress: &mut dyn FnMut(u64),
    ) -> Result<(), TransportError> {
        self.with_conn(|s| {
            s.retr(path, |reader: &mut dyn Read| {
                let mut total: u64 = 0;
                let mut buf = [0u8; 64 * 1024];
                loop {
                    let n = reader
                        .read(&mut buf)
                        .map_err(suppaftp::FtpError::ConnectionError)?;
                    if n == 0 {
                        break;
                    }
                    sink.write_all(&buf[..n])
                        .map_err(suppaftp::FtpError::ConnectionError)?;
                    total += n as u64;
                    progress(total);
                }
                Ok(())
            })
            .map_err(map_err)
        })
    }
}
