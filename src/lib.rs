#![forbid(unsafe_code)]

pub mod api;
pub mod auth;
pub mod canary;
pub mod cli;
pub mod config;
pub mod error;
pub mod hub;
mod hub_failure;
pub mod metrics;
pub mod recordings;
pub mod remux;
pub mod scenetrove;
mod scenetrove_receipt;
pub mod snapshot;
pub mod storage;
pub mod transport;
pub mod vtm;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
