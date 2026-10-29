//! Camera validation and bounded environment values.

use super::*;

#[test]
fn aliases_are_strictly_validated() {
    let mut cameras = BTreeMap::new();
    cameras.insert(
        "../serial".into(),
        CameraConfig {
            serial: "secret".into(),
            channel: 1,
            substream: false,
            decrypt_video: false,
            media_key_file: None,
        },
    );
    assert!(validate_cameras(&cameras).is_err());
}

#[test]
fn missing_or_invalid_values_use_defaults() {
    assert_eq!(
        clamped("EZVIZ_BRIDGE_TEST_UNSET_ALARM_LIMIT", 200, 50, 2000),
        200
    );
    assert_eq!(bounded(None, 200, 50, 2000), (200, false));
    assert_eq!(bounded(Some("abc"), 256, 64, 2048), (256, true));
    assert_eq!(bounded(Some("-5"), 200, 50, 2000), (200, true));
    assert_eq!(bounded(Some(" 300 "), 200, 50, 2000), (300, false));
}

#[test]
fn out_of_range_values_are_clamped() {
    assert_eq!(bounded(Some("0"), 200, 50, 2000), (50, true));
    assert_eq!(bounded(Some("999999"), 200, 50, 2000), (2000, true));
    assert_eq!(bounded(Some("1"), 256, 64, 2048), (64, true));
    assert_eq!(bounded(Some("100000"), 256, 64, 2048), (2048, true));
    assert_eq!(bounded(Some("2000"), 200, 50, 2000), (2000, false));
}

#[test]
fn alarm_limits_default_to_the_feed_defaults() {
    assert_eq!(alarm_limits(), crate::alarms::AlarmLimits::default());
}
