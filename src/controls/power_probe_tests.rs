//! Tests for the observation-only power and field probe.

use serde_json::json;

use super::*;

fn data(device: Value, status: Value) -> CameraData {
    CameraData {
        device,
        status,
        ..CameraData::default()
    }
}

#[test]
fn finds_power_fields_in_every_scope_and_json_string_optionals() {
    let camera = data(
        json!({"deviceSerial": "BA2595538", "powerStatus": 1, "status": 1}),
        json!({"globalStatus": 0, "optionals": "{\"powerRemaining\": 100, \"chargingState\": 2}"}),
    );
    let fields = power_fields(&camera);
    assert_eq!(fields.get("deviceInfos.powerStatus"), Some(&json!(1)));
    assert_eq!(
        fields.get("STATUS.optionals.powerRemaining"),
        Some(&json!(100))
    );
    assert_eq!(
        fields.get("STATUS.optionals.chargingState"),
        Some(&json!(2))
    );
    assert!(!fields.contains_key("deviceInfos.status"));
}

#[test]
fn never_reports_secret_looking_keys() {
    let camera = data(
        json!({"powerKey": "s3cr3t", "batteryToken": "t", "deviceSerial": "X"}),
        json!({"encryptPwd": "x", "optionals": {"chargeCode": "9", "powerRemaining": 50}}),
    );
    let fields = power_fields(&camera);
    assert_eq!(fields.len(), 1, "{fields:?}");
    let names: Vec<String> = key_inventory(&camera).into_values().flatten().collect();
    assert!(
        names.iter().all(|name| !name.contains("Key")
            && !name.contains("Token")
            && !name.contains("Pwd")
            && !name.contains("Code")),
        "{names:?}"
    );
}

#[test]
fn observes_plain_fields_but_never_identifying_or_volatile_ones() {
    let camera = data(
        json!({"deviceSerial": "BA2595538", "status": 1, "name": "Door", "netIp": "10.0.0.2"}),
        json!({"optionals": "{\"magnetStatus\": \"1\", \"wifiSsid\": \"home\", \"updateTime\": 5, \"lockKey\": \"x\"}"}),
    );
    let fields = observable_fields(&camera);
    let names: Vec<&str> = fields.keys().map(String::as_str).collect();
    assert_eq!(
        names,
        ["STATUS.optionals.magnetStatus", "deviceInfos.status"]
    );
}

#[test]
fn reports_only_changed_added_and_removed_fields() {
    let before = BTreeMap::from([
        ("a".to_owned(), json!("1")),
        ("b".to_owned(), json!(2)),
        ("c".to_owned(), json!(true)),
    ]);
    let after = BTreeMap::from([
        ("a".to_owned(), json!("1")),
        ("b".to_owned(), json!(3)),
        ("d".to_owned(), json!(0)),
    ]);
    let diff = changes(&before, &after);
    assert_eq!(diff.len(), 3);
    assert_eq!(diff["b"], "2 -> 3");
    assert_eq!(diff["c"], "true -> null");
    assert_eq!(diff["d"], "null -> 0");
}

#[test]
fn identifies_devices_by_serial_tail_only() {
    assert_eq!(tail("BA2595538"), "5538");
    assert_eq!(tail("AB"), "AB");
}
