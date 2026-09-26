
//! Win32 config crate (MVP subset).
//!
//! Phase-2 of the vpn-rust-native-runtime-mvp architecture: the coordinator
//! (core) reads `config.json` (target/routes/credential ciphertext), decrypts
//! the credential one-shot using the independent `key.bin`, and hands the
//! plaintext over the control-plane pipe to the engine. This crate owns that
//! file format and the AES-256-GCM credential sealing.
//!
//! Files (under `%USERPROFILE%\.exv` by default):
//!
//! - `config.json` — [`ExvConfig`] user configuration
//! - `key.bin`     — 32-byte AES key (key separation; never embedded in config)

mod config;
mod config_hygiene;
mod crypto;
mod error;
mod paths;

pub use config::{ConnectionMode, ExvConfig};
pub use config_hygiene::{StartupConfig, load_for_startup, save_after_user_submission};
pub use error::ConfigError;
pub use paths::{config_dir, config_path, key_path};

