//! Diagnostic: exercise the real release server with the transport's chosen
//! strategy (one reused control connection + Extended Passive), running the
//! installer's access pattern — list hosts, list targets, retr an archive — with
//! progress. Handy for re-checking connectivity without a full install.
//!
//! Run: `cargo run --example ftp_probe -p xenolith-installer-core --features ftp`

#[cfg(feature = "ftp")]
mod probe {
    use std::io::Read;
    use std::net::{SocketAddr, TcpStream};
    use std::time::{Duration, Instant};

    use suppaftp::{
        types::{FileType, Mode},
        FtpError, FtpStream,
    };

    const HOST: &str = "stappler.dev:21";
    const HOSTS_DIR: &str = "/releases/sdk-v0alpha0/hosts/";
    const TARGETS_DIR: &str = "/releases/sdk-v0alpha0/targets/";
    const ARCHIVE: &str = "/releases/sdk-v0alpha0/targets/armv7a-unknown-linux-androideabi.tar.xz";

    fn data_connect(addr: SocketAddr) -> Result<TcpStream, FtpError> {
        eprintln!("    data connect -> {addr}");
        TcpStream::connect_timeout(&addr, Duration::from_secs(15))
            .map_err(FtpError::ConnectionError)
    }

    pub fn run() {
        eprintln!("== reused connection + EPSV: list hosts, list targets, retr archive ==");
        let t = Instant::now();
        let res = (|| -> Result<(), FtpError> {
            let mut s = FtpStream::connect(HOST)?.passive_stream_builder(data_connect);
            s.login("anonymous", "anonymous@")?;
            s.set_mode(Mode::ExtendedPassive);
            s.transfer_type(FileType::Binary)?;
            eprintln!("  list hosts -> {} entries", s.list(Some(HOSTS_DIR))?.len());
            eprintln!(
                "  list targets -> {} entries",
                s.list(Some(TARGETS_DIR))?.len()
            );
            eprintln!("  retr archive...");
            let mut total = 0u64;
            let mut mark = 4_000_000u64;
            s.retr(ARCHIVE, |r: &mut dyn Read| {
                let mut buf = [0u8; 64 * 1024];
                loop {
                    let n = r.read(&mut buf).map_err(FtpError::ConnectionError)?;
                    if n == 0 {
                        break;
                    }
                    total += n as u64;
                    if total >= mark {
                        eprintln!("    {} MB...", total / 1_000_000);
                        mark += 4_000_000;
                    }
                }
                Ok(())
            })?;
            let _ = s.quit();
            eprintln!("  retrieved {total} bytes");
            Ok(())
        })();
        match res {
            Ok(()) => eprintln!("  OK in {:.1?}", t.elapsed()),
            Err(e) => eprintln!("  FAIL: {e} after {:.1?}", t.elapsed()),
        }
    }
}

#[cfg(feature = "ftp")]
fn main() {
    probe::run();
}

#[cfg(not(feature = "ftp"))]
fn main() {
    eprintln!("rebuild with --features ftp");
}
