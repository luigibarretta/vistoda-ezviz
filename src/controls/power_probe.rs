//! Observation-only probe for the vendor power/charging fields.
//!
//! EZVIZ does not document a charging flag and neither pyezvizapi nor this
//! bridge parses one (only `powerRemaining` and `batteryCameraWorkMode`).
//! Before exposing a charging state, this probe records what the cloud really
//! sends: once per process the key *names* of `deviceInfos`, `STATUS` and
//! `STATUS.optionals`, and on every change the *values* of power-related keys
//! only. Keys that look secret are never logged; devices are identified by the
//! last four serial characters. It reads the snapshot already fetched for the
//! controls and makes no extra cloud call.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use serde_json::{Map, Value};

use super::{CameraData, Snapshot};

const POWER_HINTS: [&str; 9] = [
    "power", "charg", "batt", "plug", "electr", "supply", "adapter", "solar", "cable",
];
const SECRET_HINTS: [&str; 6] = ["pwd", "password", "key", "token", "secret", "code"];

fn is_secret(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_HINTS.iter().any(|hint| lower.contains(hint))
}

fn is_power(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    !is_secret(key) && POWER_HINTS.iter().any(|hint| lower.contains(hint))
}

/// `STATUS.optionals`, which EZVIZ may send as a JSON-encoded string.
fn optionals(status: &Value) -> Option<Map<String, Value>> {
    match status.get("optionals")? {
        Value::Object(map) => Some(map.clone()),
        Value::String(text) => serde_json::from_str::<Map<String, Value>>(text).ok(),
        _ => None,
    }
}

/// The three inspected scopes of one device: name -> object.
fn scopes(data: &CameraData) -> Vec<(&'static str, Map<String, Value>)> {
    let mut found = Vec::new();
    if let Some(device) = data.device.as_object() {
        found.push(("deviceInfos", device.clone()));
    }
    if let Some(status) = data.status.as_object() {
        found.push(("STATUS", status.clone()));
    }
    if let Some(map) = optionals(&data.status) {
        found.push(("STATUS.optionals", map));
    }
    found
}

/// Power-related scalar values, keyed `scope.key`.
#[must_use]
pub fn power_fields(data: &CameraData) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::new();
    for (scope, map) in scopes(data) {
        for (key, value) in map {
            if is_power(&key) && !value.is_object() && !value.is_array() {
                fields.insert(format!("{scope}.{key}"), value);
            }
        }
    }
    fields
}

/// Non-secret key names per scope (values are never included).
#[must_use]
pub fn key_inventory(data: &CameraData) -> BTreeMap<&'static str, BTreeSet<String>> {
    scopes(data)
        .into_iter()
        .map(|(scope, map)| {
            (
                scope,
                map.keys().filter(|key| !is_secret(key)).cloned().collect(),
            )
        })
        .collect()
}

fn tail(serial: &str) -> String {
    let chars: Vec<char> = serial.chars().collect();
    chars[chars.len().saturating_sub(4)..].iter().collect()
}

#[derive(Default)]
struct Seen {
    inventory: BTreeSet<String>,
    fields: HashMap<String, BTreeMap<String, Value>>,
}

fn seen() -> &'static Mutex<Seen> {
    static SEEN: OnceLock<Mutex<Seen>> = OnceLock::new();
    SEEN.get_or_init(Mutex::default)
}

/// Logs the key inventory once per device and power fields when they change.
pub fn observe(snapshot: &Snapshot) {
    let Ok(mut state) = seen().lock() else {
        return;
    };
    for (serial, data) in snapshot {
        let device = tail(serial);
        if state.inventory.insert(serial.clone()) {
            for (scope, keys) in key_inventory(data) {
                let names = keys.into_iter().collect::<Vec<_>>().join(",");
                tracing::info!(%device, scope, keys = %names, "power probe: field names");
            }
        }
        let fields = power_fields(data);
        if state.fields.get(serial) != Some(&fields) {
            let rendered = serde_json::to_string(&fields).unwrap_or_default();
            tracing::info!(%device, fields = %rendered, "power probe: power fields changed");
            state.fields.insert(serial.clone(), fields);
        }
    }
}

#[cfg(test)]
mod tests {
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
    fn identifies_devices_by_serial_tail_only() {
        assert_eq!(tail("BA2595538"), "5538");
        assert_eq!(tail("AB"), "AB");
    }
}
