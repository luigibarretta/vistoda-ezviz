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
//!
//! Since 0.13.1 it also records which other fields change (for example when a
//! door viewer is mounted on its door base), so a physical action can be
//! correlated with a cloud field. Keys that look secret, identifying (address,
//! network, name, location, account) or merely volatile (time, signal) are
//! never logged.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use serde_json::{Map, Value};

use super::{CameraData, Snapshot};

const POWER_HINTS: [&str; 9] = [
    "power", "charg", "batt", "plug", "electr", "supply", "adapter", "solar", "cable",
];
const SECRET_HINTS: [&str; 6] = ["pwd", "password", "key", "token", "secret", "code"];
const PRIVATE_HINTS: [&str; 16] = [
    "ip", "mac", "ssid", "serial", "name", "url", "addr", "lat", "lon", "locat", "mail", "phone",
    "user", "account", "host", "domain",
];
const VOLATILE_HINTS: [&str; 6] = ["time", "stamp", "signal", "rssi", "uptime", "date"];

fn is_secret(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    SECRET_HINTS.iter().any(|hint| lower.contains(hint))
}

fn is_power(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    !is_secret(key) && POWER_HINTS.iter().any(|hint| lower.contains(hint))
}

fn is_observable(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    !is_secret(key)
        && !PRIVATE_HINTS.iter().any(|hint| lower.contains(hint))
        && !VOLATILE_HINTS.iter().any(|hint| lower.contains(hint))
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

/// Every observable scalar value, keyed `scope.key`.
#[must_use]
pub fn observable_fields(data: &CameraData) -> BTreeMap<String, Value> {
    let mut fields = BTreeMap::new();
    for (scope, map) in scopes(data) {
        for (key, value) in map {
            // The raw `optionals` string is decomposed into its own scope;
            // logging it whole would bypass the per-key filters.
            let raw_optionals = scope == "STATUS" && key == "optionals";
            if !raw_optionals && is_observable(&key) && !value.is_object() && !value.is_array() {
                fields.insert(format!("{scope}.{key}"), value);
            }
        }
    }
    fields
}

/// `key: old -> new` for every changed, added or removed field.
#[must_use]
pub fn changes(
    before: &BTreeMap<String, Value>,
    after: &BTreeMap<String, Value>,
) -> BTreeMap<String, String> {
    let missing = Value::Null;
    before
        .keys()
        .chain(after.keys())
        .filter(|key| before.get(*key) != after.get(*key))
        .map(|key| {
            let old = before.get(key).unwrap_or(&missing);
            let new = after.get(key).unwrap_or(&missing);
            (key.clone(), format!("{old} -> {new}"))
        })
        .collect()
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
    observed: HashMap<String, BTreeMap<String, Value>>,
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
        let observed = observable_fields(data);
        match state.observed.get(serial) {
            None => {
                let rendered = serde_json::to_string(&observed).unwrap_or_default();
                tracing::info!(%device, fields = %rendered, "field probe: baseline");
            }
            Some(previous) if previous != &observed => {
                let rendered =
                    serde_json::to_string(&changes(previous, &observed)).unwrap_or_default();
                tracing::info!(%device, changes = %rendered, "field probe: fields changed");
            }
            Some(_) => continue,
        }
        state.observed.insert(serial.clone(), observed);
    }
}

#[cfg(test)]
#[path = "power_probe_tests.rs"]
mod tests;
