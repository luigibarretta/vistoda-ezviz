//! Synthetic resource-list fixtures for the control read model.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::*;

fn serials(list: &[&str]) -> BTreeSet<String> {
    list.iter().map(|serial| (*serial).to_owned()).collect()
}

fn page_one() -> Value {
    json!({
        "page": {"hasNext": true},
        "deviceInfos": [
            {"deviceSerial": "CAM1", "status": 1, "version": "V5.3.8 build 240101",
             "supportExt": "{\"1\":\"1\",\"40\":\"1\",\"61\":\"3\",\"154\":\"1\",\"62\":\"1\"}"},
            {"deviceSerial": "OTHER", "status": 1, "supportExt": "{}"}
        ],
        "STATUS": {
            "CAM1": {"globalStatus": 1, "encryptPwd": "0123456789abcdef0123456789abcdef",
                     "optionals": "{\"Alarm_DetectHumanCar\":\"{\\\"type\\\":3}\",\"powerRemaining\":\"87\",\"batteryCameraWorkMode\":1}"},
            "OTHER": {"globalStatus": 0}
        }
    })
}

fn page_two() -> Value {
    json!({
        "page": {"hasNext": false},
        "deviceInfos": [{"deviceSerial": "CAM2", "status": 2, "supportExt": {"1": "0"}}],
        "STATUS": {"CAM2": {"globalStatus": 1, "optionals": {"Alarm_DetectHumanCar": {"type": 9}}}},
        "SWITCH": {
            "CAM1": [
                {"type": 7, "enable": true}, {"type": 21, "enable": 0},
                {"type": 3, "enable": "1"}, {"type": 10, "enable": true},
                {"type": 458, "enable": true}, {"type": 604, "enable": false}
            ],
            "CAM2": [{"type": 702, "enable": 1}, {"type": 200, "enable": "maybe"}]
        },
        "TIME_PLAN": {"CAM1": [{"type": 1, "enable": 0}, {"type": 2, "enable": 1}]},
        "UPGRADE": {"CAM1": {"isNeedUpgrade": 3}, "CAM2": {"isNeedUpgrade": 0}}
    })
}

fn snapshot() -> Snapshot {
    let mut snapshot = Snapshot::new();
    let wanted = serials(&["CAM1", "CAM2"]);
    merge_page(&mut snapshot, &page_one(), &wanted);
    merge_page(&mut snapshot, &page_two(), &wanted);
    snapshot
}

#[test]
fn pages_merge_only_configured_serials_and_drop_the_code_hash() {
    let snapshot = snapshot();
    assert_eq!(snapshot.keys().collect::<Vec<_>>(), ["CAM1", "CAM2"]);
    assert!(page_has_next(&page_one()));
    assert!(!page_has_next(&page_two()));
    assert!(!page_has_next(&json!({})));
    let status = &snapshot["CAM1"].status;
    assert!(status.get("encryptPwd").is_none());
    assert_eq!(status.get("globalStatus"), Some(&json!(1)));
}

#[test]
fn controls_follow_the_contract_for_a_full_camera() {
    let snapshot = snapshot();
    let data = &snapshot["CAM1"];
    let controls = controls_from(data, None);
    assert_eq!(controls.online, Some(true));
    assert_eq!(controls.defence_enabled, Some(true));
    assert_eq!(controls.alarm_schedule_enabled, Some(true));
    assert_eq!(controls.detection_mode, Some(DetectionMode::ImageChange));
    assert!(controls.ptz);
    // 10 needs supportExt "48", 458 (remote unlock) is never exposed.
    let switches: Vec<_> = controls.switches.iter().map(|(k, v)| (*k, *v)).collect();
    assert_eq!(
        switches,
        [
            ("privacy", true),
            ("sleep", false),
            ("status_light", true),
            ("wdr", false)
        ]
    );
    let battery = controls.battery.unwrap_or_else(|| panic!("battery"));
    assert_eq!(battery.percent, Some(87));
    assert_eq!(battery.work_mode, Some("high_performance"));
    let firmware = controls.firmware.unwrap_or_else(|| panic!("firmware"));
    assert_eq!(firmware.version.as_deref(), Some("V5.3.8 build 240101"));
    assert_eq!(firmware.update_available, Some(true));
    assert_eq!(sensitivity_kind(&data.device), Some(3));
}

#[test]
fn unsupported_or_malformed_fields_degrade_to_null() {
    let snapshot = snapshot();
    let controls = controls_from(&snapshot["CAM2"], None);
    assert_eq!(controls.online, Some(false));
    // supportExt "1" == "0": no per-camera arming.
    assert_eq!(controls.defence_enabled, None);
    assert_eq!(controls.alarm_schedule_enabled, None);
    assert_eq!(controls.detection_mode, None);
    assert!(!controls.ptz);
    assert_eq!(controls.battery, None);
    assert_eq!(controls.switches.get("logo"), Some(&true));
    assert!(!controls.switches.contains_key("human_detection"));
    let firmware = controls.firmware.unwrap_or_else(|| panic!("firmware"));
    assert_eq!(firmware.version, None);
    assert_eq!(firmware.update_available, Some(false));
    let unknown = controls_from(&CameraData::default(), None);
    assert_eq!(
        serde_json::to_value(&unknown).ok(),
        Some(json!({
            "online": null, "defence_enabled": null, "alarm_schedule_enabled": null,
            "detection_mode": null, "sensitivity": null, "switches": {}, "ptz": false,
            "battery": null, "firmware": null
        }))
    );
}

#[test]
fn sensitivity_uses_the_capability_type_channel_and_range() {
    let raw = json!({"resultCode": "0", "algorithmConfig": {"algorithmList": [
        {"type": "0", "value": "4", "channel": 1},
        {"type": "3", "value": "55", "channel": 2},
        {"type": "3", "value": "70", "channel": 1}
    ]}});
    let level = parse_sensitivity(&raw, 3, 1).unwrap_or_else(|| panic!("level"));
    assert_eq!(
        (level.value, level.min, level.max, level.kind),
        (70, 0, 100, 3)
    );
    let body = serde_json::to_value(level).unwrap_or_default();
    assert_eq!(body, json!({"value": 70, "min": 0, "max": 100}));
    assert_eq!(
        parse_sensitivity(&raw, 0, 1).map(|s| (s.value, s.max)),
        Some((4, 6))
    );
    assert_eq!(parse_sensitivity(&raw, 3, 2).map(|s| s.value), Some(55));
    let offline = json!({"resultCode": "-1", "algorithmConfig": {"algorithmList": []}});
    assert!(parse_sensitivity(&offline, 3, 1).is_none());
    let out_of_range = json!({"resultCode": 0, "algorithmConfig": {"algorithmList": [
        {"type": 0, "value": 9}]}});
    assert!(parse_sensitivity(&out_of_range, 0, 1).is_none());
    assert_eq!(
        sensitivity_kind(&json!({"supportExt": {"61": "1"}})),
        Some(0)
    );
    assert_eq!(sensitivity_kind(&json!({"supportExt": {"61": "2"}})), None);
}

#[test]
fn group_defence_modes_map_both_ways() {
    for (code, mode) in [
        ("1", DefenceMode::Home),
        ("2", DefenceMode::Away),
        ("3", DefenceMode::Sleep),
    ] {
        assert_eq!(parse_group_mode(&json!({"mode": code})), Some(mode));
        assert_eq!(
            i64::from(mode.code()),
            code.parse::<i64>().unwrap_or_default()
        );
    }
    assert_eq!(parse_group_mode(&json!({"mode": "0"})), None);
    assert_eq!(parse_group_mode(&json!({})), None);
    for mode in [
        DetectionMode::HumanShape,
        DetectionMode::ImageChange,
        DetectionMode::Pir,
    ] {
        assert_eq!(DetectionMode::from_code(i64::from(mode.code())), Some(mode));
    }
}

#[test]
fn switch_names_are_unique_and_stable() {
    let names: BTreeSet<_> = SWITCHES.iter().map(|spec| spec.name).collect();
    let kinds: BTreeSet<_> = SWITCHES.iter().map(|spec| spec.kind).collect();
    assert_eq!(names.len(), SWITCHES.len());
    assert_eq!(kinds.len(), SWITCHES.len());
    for name in [
        "privacy",
        "sleep",
        "status_light",
        "infrared_light",
        "human_detection",
    ] {
        assert!(names.contains(name), "{name}");
    }
    assert!(names.iter().all(|name| {
        name.bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
    }));
}
