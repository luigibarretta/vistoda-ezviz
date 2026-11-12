//! Shared Rust building blocks of the Vistoda provider bridges (Blink, EZVIZ
//! and Ring).
//!
//! - [`private_file`]: checked reads of owner-private files and atomic private
//!   writes, on top of `auth_modules::secrets::files`.
//! - [`sealed_store`]: AES-256-GCM sealed files (`{version, nonce,
//!   ciphertext}`) keyed by an HKDF sub-key of a dedicated `store.key`, with
//!   legacy-key and plaintext migration.
//! - [`auth`]: constant-time bearer (and optional Home Assistant Basic)
//!   checks and an axum middleware that authenticates a whole router.
//!
//! Providers keep their own persistence formats, routes and error mapping.

#![cfg(unix)]

pub mod auth;
pub mod private_file;
pub mod sealed_store;

pub use auth_modules::keys::KeyLabel;
