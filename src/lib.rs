#![forbid(unsafe_code)]

pub mod api;
mod api_recordings;
pub mod auth;
pub mod bootstrap;
pub mod canary;
pub mod cli;
pub mod config;
mod enrollment;
pub mod error;
pub mod hub;
mod hub_failure;
pub mod metrics;
mod pagination;
mod recording_playback;
pub mod recordings;
pub mod remux;
pub mod scenetrove;
mod scenetrove_receipt;
pub mod snapshot;
pub mod storage;
pub mod transport;
pub mod vtm;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
