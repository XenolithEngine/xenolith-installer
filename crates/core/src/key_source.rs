//! Fetch the pinned release public key from a keyserver (feature `keyserver`).
//!
//! The key is NOT embedded in the binary. It is fetched over HTTPS at install
//! time and only trusted if its fingerprint matches the pin in
//! [`crate::verify::RELEASE_KEY_FINGERPRINT`] — see [`crate::verify::PgpVerifier`].
//! That fingerprint check is what makes fetching from a public keyserver safe.

/// Default keyserver. `keyserver.ubuntu.com` serves armored keys over HTTPS via
/// the HKP `op=get` endpoint (`options=mr` → machine-readable, no HTML).
pub const DEFAULT_KEYSERVER: &str = "https://keyserver.ubuntu.com";

#[derive(Debug, thiserror::Error)]
pub enum KeyFetchError {
    #[error("keyserver request failed: {0}")]
    Http(String),
}

/// Build the HKP lookup URL for a fingerprint on a given keyserver base.
pub fn lookup_url(keyserver_base: &str, fingerprint: &str) -> String {
    format!(
        "{}/pks/lookup?op=get&options=mr&search=0x{}",
        keyserver_base.trim_end_matches('/'),
        fingerprint
    )
}

/// Fetch the armored release key from `keyserver_base`.
#[cfg(feature = "keyserver")]
pub fn fetch_from(keyserver_base: &str) -> Result<String, KeyFetchError> {
    let url = lookup_url(keyserver_base, crate::verify::RELEASE_KEY_FINGERPRINT);
    ureq::get(&url)
        .call()
        .map_err(|e| KeyFetchError::Http(e.to_string()))?
        .into_string()
        .map_err(|e| KeyFetchError::Http(e.to_string()))
}

/// Fetch the armored release key from the default keyserver.
#[cfg(feature = "keyserver")]
pub fn fetch_release_key() -> Result<String, KeyFetchError> {
    fetch_from(DEFAULT_KEYSERVER)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lookup_url_is_well_formed_and_pins_the_fingerprint() {
        let url = lookup_url(DEFAULT_KEYSERVER, crate::verify::RELEASE_KEY_FINGERPRINT);
        assert_eq!(
            url,
            "https://keyserver.ubuntu.com/pks/lookup?op=get&options=mr&search=0xD35FB3717A2DCA13E5722EB850243B02EB5F7F73"
        );
    }

    #[test]
    fn trailing_slash_on_base_is_handled() {
        let url = lookup_url("https://ks.example/", "ABCD");
        assert_eq!(
            url,
            "https://ks.example/pks/lookup?op=get&options=mr&search=0xABCD"
        );
    }
}
