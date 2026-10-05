//! Native camera controls that replace Home Assistant's `ezviz` integration.
//!
//! Reads come from one cached, account-wide resource-list fetch (plus a
//! cached per-camera sensitivity read for capable cameras). Writes happen only
//! on explicit API calls, are checked against the caller's expected value,
//! verified by reading back and rolled back once when the read-back disagrees.
//! Nothing here can reach the cloud verification-code lookup: the source trait
//! has no such method and the service never holds a `DeviceSource`.

mod account;
mod cell;
mod error;
mod fields;
mod model;
mod names;
mod service;
mod source;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_write;
mod update;
mod write;

pub use account::{AccountDefence, DefenceMode, DefenceUpdate, parse_group_mode};
pub use error::ControlError;
pub use model::{
    Battery, CameraData, Controls, DetectionMode, Firmware, Sensitivity, Snapshot, controls_from,
    merge_page, page_has_next, parse_sensitivity, sensitivity_kind,
};
pub use names::{SWITCHES, SwitchSpec};
pub use service::ControlService;
pub use source::{ControlSource, PtzAction, PtzDirection, VendorWrite};
pub use write::{ControlRequest, PtzRequest};

use std::time::Duration;

/// Freshness of the account-wide control snapshot and of sensitivity reads.
pub const CONTROLS_TTL: Duration = Duration::from_secs(60);
/// Failed control reads are retried no sooner than this.
pub const CONTROLS_FAILURE_TTL: Duration = Duration::from_secs(30);
/// Freshness of the account-level defence mode.
pub const DEFENCE_TTL: Duration = Duration::from_secs(60);
/// Waits before each read-back after a write; bounded, no recursion.
pub const VERIFY_DELAYS: [Duration; 3] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(3),
];
/// Resource-list page size and page bound, as used by the status lookup.
pub const PAGE_SIZE: usize = 50;
pub const MAX_PAGES: usize = 40;
