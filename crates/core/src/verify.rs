//! Signature verification.
//!
//! SDK artifacts carry a detached binary OpenPGP signature (`*.tar.xz.sig`,
//! RSA + SHA-512). We do NOT embed the public key in the binary; instead the
//! **fingerprint** is pinned here as the trust anchor and the key itself is
//! fetched at runtime (see [`crate::key_source`]) from a public keyserver. A
//! fetched key is trusted only if its fingerprint matches the pin, so a
//! compromised keyserver or a MITM cannot substitute a different key.
//!
//! Everything here is pure (no network): it takes an armored key string and
//! verifies in-process via rPGP. The pipeline fails closed — any error means
//! "do not install".

use pgp::types::PublicKeyTrait;
use pgp::{Deserializable, SignedPublicKey, StandaloneSignature};

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error("signature does not match the trusted key")]
    BadSignature,
    #[error("untrusted key: fingerprint {found} does not match the pinned anchor")]
    UntrustedKey { found: String },
    #[error("could not parse public key: {0}")]
    KeyParse(String),
    #[error("could not parse signature: {0}")]
    SignatureParse(String),
    #[error("verifier error: {0}")]
    Other(String),
}

pub trait Verifier {
    /// Check that `detached_sig` is a valid signature over `data` from the
    /// trusted key. `Ok(())` means trusted; any `Err` means do NOT install.
    fn verify(&self, data: &[u8], detached_sig: &[u8]) -> Result<(), VerifyError>;
}

/// Pinned fingerprint of the Xenolith release signing key (the trust anchor).
/// A fingerprint is public by design; this is not the key, just the anchor a
/// fetched key is checked against.
pub const RELEASE_KEY_FINGERPRINT: &str = "D35FB3717A2DCA13E5722EB850243B02EB5F7F73";

fn fingerprint_hex(key: &SignedPublicKey) -> String {
    key.fingerprint()
        .as_bytes()
        .iter()
        .fold(String::new(), |mut s, b| {
            use std::fmt::Write;
            let _ = write!(s, "{b:02X}");
            s
        })
}

fn normalize_fpr(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_hexdigit())
        .flat_map(|c| c.to_uppercase())
        .collect()
}

/// Real verifier: validates detached OpenPGP signatures against a public key
/// whose fingerprint matched the pin.
pub struct PgpVerifier {
    key: SignedPublicKey,
}

impl PgpVerifier {
    /// Parse an armored public key, self-verify it, and require its fingerprint
    /// to equal `expected_fpr`. Fails closed on any mismatch or parse error.
    pub fn from_armored_pinned(armored: &str, expected_fpr: &str) -> Result<Self, VerifyError> {
        let (key, _) = SignedPublicKey::from_armor_single(armored.as_bytes())
            .map_err(|e| VerifyError::KeyParse(e.to_string()))?;
        key.verify()
            .map_err(|e| VerifyError::KeyParse(format!("self-verify: {e}")))?;
        let found = fingerprint_hex(&key);
        if found != normalize_fpr(expected_fpr) {
            return Err(VerifyError::UntrustedKey { found });
        }
        Ok(PgpVerifier { key })
    }

    /// Build the verifier for the pinned Xenolith release key.
    pub fn release(armored: &str) -> Result<Self, VerifyError> {
        Self::from_armored_pinned(armored, RELEASE_KEY_FINGERPRINT)
    }
}

impl Verifier for PgpVerifier {
    fn verify(&self, data: &[u8], detached_sig: &[u8]) -> Result<(), VerifyError> {
        let sig = StandaloneSignature::from_bytes(detached_sig)
            .map_err(|e| VerifyError::SignatureParse(e.to_string()))?;
        sig.verify(&self.key, data)
            .map_err(|_| VerifyError::BadSignature)
    }
}

/// INSECURE. Accepts any signature. For tests and explicit dev/offline modes
/// only — never select this in a release build path.
pub struct AcceptAll;

impl Verifier for AcceptAll {
    fn verify(&self, _data: &[u8], _detached_sig: &[u8]) -> Result<(), VerifyError> {
        Ok(())
    }
}

/// Rejects everything. Safe default; used in tests asserting fail-closed.
pub struct RejectAll;

impl Verifier for RejectAll {
    fn verify(&self, _data: &[u8], _detached_sig: &[u8]) -> Result<(), VerifyError> {
        Err(VerifyError::BadSignature)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The real release public key, as a TEST fixture only. Pulled into the test
    // binary via include_str! — NOT compiled into the shipped installer (the key
    // is fetched at runtime in production).
    const RELEASE_KEY: &str = include_str!("../tests/fixtures/xenolith-release.asc");

    #[test]
    fn pinned_fingerprint_is_40_hex_chars() {
        assert_eq!(RELEASE_KEY_FINGERPRINT.len(), 40);
        assert!(RELEASE_KEY_FINGERPRINT
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn release_key_parses_and_matches_the_pin() {
        // The fetched key's fingerprint must equal the pinned anchor.
        let v = PgpVerifier::release(RELEASE_KEY);
        assert!(v.is_ok(), "release key should match pin: {:?}", v.err());
    }

    #[test]
    fn key_with_wrong_pin_is_untrusted() {
        let bogus = "0000000000000000000000000000000000000000";
        match PgpVerifier::from_armored_pinned(RELEASE_KEY, bogus) {
            Err(VerifyError::UntrustedKey { .. }) => {}
            _ => panic!("a mismatched fingerprint must be rejected as untrusted"),
        }
    }

    #[test]
    fn garbage_key_is_rejected() {
        assert!(matches!(
            PgpVerifier::release("not a key"),
            Err(VerifyError::KeyParse(_))
        ));
    }

    #[test]
    fn garbage_signature_is_rejected_not_accepted() {
        let v = PgpVerifier::release(RELEASE_KEY).unwrap();
        // Random bytes are not a valid signature packet → must error, never Ok.
        assert!(v.verify(b"data", b"\x00\x01\x02 not a signature").is_err());
    }

    #[test]
    fn accept_all_and_reject_all_contract() {
        assert!(AcceptAll.verify(b"x", b"y").is_ok());
        assert!(matches!(
            RejectAll.verify(b"x", b"y"),
            Err(VerifyError::BadSignature)
        ));
    }
}
