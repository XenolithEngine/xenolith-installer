//! Verify a real, locally-present archive against its detached `.sig` using the
//! pinned release key. Proves end-to-end GPG verification on a genuine artifact
//! without going through the (currently throttled) FTP transport.
//!
//! Usage: `cargo run --example verify_probe -p xenolith-installer-core -- <archive> <sig>`

use xenolith_installer_core::verify::{PgpVerifier, Verifier};

// The real release key (test/diagnostic fixture; not the shipped trust path —
// production fetches it from a keyserver and pins the same fingerprint).
const RELEASE_KEY: &str = include_str!("../tests/fixtures/xenolith-release.asc");

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(archive), Some(sig)) = (args.next(), args.next()) else {
        eprintln!("usage: verify_probe <archive> <sig>");
        std::process::exit(2);
    };

    let verifier = match PgpVerifier::release(RELEASE_KEY) {
        Ok(v) => {
            println!("key OK: parsed and fingerprint matches the pin");
            v
        }
        Err(e) => {
            eprintln!("key FAIL: {e}");
            std::process::exit(1);
        }
    };

    let data = std::fs::read(&archive).expect("read archive");
    let signature = std::fs::read(&sig).expect("read sig");
    println!(
        "archive: {} bytes, sig: {} bytes",
        data.len(),
        signature.len()
    );

    match verifier.verify(&data, &signature) {
        Ok(()) => println!("SIGNATURE VALID ✓ — real key verified a real server artifact"),
        Err(e) => {
            eprintln!("SIGNATURE INVALID: {e}");
            std::process::exit(1);
        }
    }
}
