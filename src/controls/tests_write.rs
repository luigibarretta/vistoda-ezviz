//! Write validation and vendor mapping.

use std::collections::BTreeMap;

use serde_json::json;

use super::{
    ControlError, Controls, DetectionMode, Sensitivity, VendorWrite,
    write::{ControlKey, Desired, current, parse_key, parse_value, vendor_write},
};

fn controls() -> Controls {
    Controls {
        online: Some(true),
        defence_enabled: Some(false),
        alarm_schedule_enabled: None,
        detection_mode: Some(DetectionMode::Pir),
        sensitivity: Some(Sensitivity {
            value: 3,
            min: 0,
            max: 6,
            kind: 0,
        }),
        switches: BTreeMap::from([("privacy", true)]),
        ptz: false,
        battery: None,
        firmware: None,
    }
}

#[test]
fn keys_are_an_allow_list() {
    assert_eq!(parse_key("defence_enabled"), Ok(ControlKey::Defence));
    assert_eq!(parse_key("detection_mode"), Ok(ControlKey::DetectionMode));
    assert_eq!(parse_key("sensitivity"), Ok(ControlKey::Sensitivity));
    assert!(matches!(
        parse_key("switch.privacy"),
        Ok(ControlKey::Switch(spec)) if spec.kind == 7
    ));
    for invalid in [
        "switch.7",
        "switch.",
        "privacy",
        "switch.remote_unlock",
        "",
        "SENSITIVITY",
    ] {
        assert!(
            matches!(parse_key(invalid), Err(ControlError::Invalid(_))),
            "{invalid}"
        );
    }
}

#[test]
fn values_are_strictly_typed() {
    let switch = parse_key("switch.privacy").unwrap_or(ControlKey::Defence);
    assert_eq!(parse_value(switch, &json!(true)), Ok(Desired::Flag(true)));
    assert_eq!(
        parse_value(ControlKey::DetectionMode, &json!("human_shape")),
        Ok(Desired::Mode(DetectionMode::HumanShape))
    );
    assert_eq!(
        parse_value(ControlKey::Sensitivity, &json!(4)),
        Ok(Desired::Level(4))
    );
    for (key, value) in [
        (ControlKey::Defence, json!(1)),
        (ControlKey::Defence, json!("true")),
        (switch, json!(null)),
        (ControlKey::DetectionMode, json!(1)),
        (ControlKey::DetectionMode, json!("motion")),
        (ControlKey::Sensitivity, json!("4")),
        (ControlKey::Sensitivity, json!(4.5)),
    ] {
        assert!(parse_value(key, &value).is_err(), "{key:?} {value}");
    }
}

#[test]
fn current_values_come_from_the_controls_object() {
    let controls = controls();
    let privacy = parse_key("switch.privacy").unwrap_or(ControlKey::Defence);
    let sleep = parse_key("switch.sleep").unwrap_or(ControlKey::Defence);
    assert_eq!(current(&controls, privacy), Some(Desired::Flag(true)));
    assert_eq!(current(&controls, sleep), None);
    assert_eq!(
        current(&controls, ControlKey::Defence),
        Some(Desired::Flag(false))
    );
    assert_eq!(
        current(&controls, ControlKey::Sensitivity),
        Some(Desired::Level(3))
    );
    assert_eq!(Desired::Mode(DetectionMode::Pir).to_json(), json!("pir"));
}

#[test]
fn writes_map_to_one_vendor_call_with_range_checks() {
    let controls = controls();
    let privacy = parse_key("switch.privacy").unwrap_or(ControlKey::Defence);
    assert_eq!(
        vendor_write(privacy, Desired::Flag(false), &controls),
        Ok(VendorWrite::Switch {
            kind: 7,
            enable: false
        })
    );
    assert_eq!(
        vendor_write(ControlKey::Defence, Desired::Flag(true), &controls),
        Ok(VendorWrite::Defence(true))
    );
    assert_eq!(
        vendor_write(
            ControlKey::DetectionMode,
            Desired::Mode(DetectionMode::ImageChange),
            &controls
        ),
        Ok(VendorWrite::DetectionMode(3))
    );
    assert_eq!(
        vendor_write(ControlKey::Sensitivity, Desired::Level(6), &controls),
        Ok(VendorWrite::Sensitivity { kind: 0, value: 6 })
    );
    assert!(matches!(
        vendor_write(ControlKey::Sensitivity, Desired::Level(7), &controls),
        Err(ControlError::Invalid(_))
    ));
    let mut without = controls;
    without.sensitivity = None;
    assert_eq!(
        vendor_write(ControlKey::Sensitivity, Desired::Level(2), &without),
        Err(ControlError::Unsupported)
    );
}
