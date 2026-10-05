//! Stable API names for EZVIZ device switch types.
//!
//! Types and capability gates mirror Home Assistant core's `ezviz` switch
//! platform (pyezvizapi 1.0.0.7 `DeviceSwitchType`) plus the image and
//! detection switches the Vistoda panel exposed (200, 604, 617, 702). Unknown
//! types are never exposed, so lock, Wi-Fi, 4G or log switches stay hidden.

/// One exposed switch: EZVIZ type, API name and optional `supportExt` key
/// that must be present for the switch to be shown (Home Assistant parity).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SwitchSpec {
    pub kind: u16,
    pub name: &'static str,
    pub capability: Option<&'static str>,
}

const fn spec(kind: u16, name: &'static str, capability: Option<&'static str>) -> SwitchSpec {
    SwitchSpec {
        kind,
        name,
        capability,
    }
}

/// Every switch the API can read or write.
pub const SWITCHES: &[SwitchSpec] = &[
    spec(3, "status_light", None),
    spec(7, "privacy", Some("40")),
    spec(10, "infrared_light", Some("48")),
    spec(21, "sleep", Some("62")),
    spec(22, "audio", Some("63")),
    spec(25, "motion_tracking", Some("73")),
    spec(29, "all_day_video_recording", Some("88")),
    spec(32, "auto_sleep", Some("144")),
    spec(200, "human_detection", None),
    spec(301, "flicker_light_on_movement", Some("96")),
    spec(305, "pir_motion_activated_light", Some("297")),
    spec(306, "tamper_alarm", Some("327")),
    spec(604, "wdr", None),
    spec(617, "distortion_correction", None),
    spec(650, "follow_movement", Some("198")),
    spec(702, "logo", None),
];

#[must_use]
pub fn by_kind(kind: u16) -> Option<&'static SwitchSpec> {
    SWITCHES.iter().find(|spec| spec.kind == kind)
}

#[must_use]
pub fn by_name(name: &str) -> Option<&'static SwitchSpec> {
    SWITCHES.iter().find(|spec| spec.name == name)
}
