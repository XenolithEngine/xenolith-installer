//! Core logic for the Xenolith Engine installer.
//!
//! UI-agnostic: no Tauri, no webview, no GTK. Both the headless CLI and the
//! Tauri GUI are thin shells over this crate. Network transport and signature
//! verification sit behind traits so they can be mocked in tests and swapped
//! for the real implementations once the signing key and HTTPS mirror are
//! finalised.

pub mod catalog;
pub mod dirs;
pub mod extract;
pub mod hash;
pub mod i18n;
pub mod install;
pub mod key_source;
pub mod manifest;
pub mod state;
pub mod transport;
#[cfg(feature = "ftp")]
pub mod transport_ftp;
pub mod triple;
pub mod verify;
