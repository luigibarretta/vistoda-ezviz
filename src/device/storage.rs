//! Mapping of the app's `/api/device/queryStorageStatus` response.
//!
//! The EZVIZ 7.6.1 storage screen (`StorageActivity`/`StorageAdapter`) reads
//! `storageStatus.storageList[]`: an empty list means "no SD card"; item
//! `status` 0 is normal (capacity shown, in MiB), 1 abnormal, 2 unformatted
//! and 3 formatting. The response carries no free-space field.

use serde::Serialize;
use serde_json::Value;

use super::model::integer;

/// Public card state; formatting in progress is reported as `unknown`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageState {
    Ok,
    NoCard,
    Unformatted,
    Error,
    Unknown,
}

/// `GET /v1/cameras/{camera}/storage` response body.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct StorageReport {
    pub status: StorageState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub capacity_mb: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_mb: Option<u64>,
}

impl StorageReport {
    #[must_use]
    pub const fn unknown() -> Self {
        Self {
            status: StorageState::Unknown,
            capacity_mb: None,
            free_mb: None,
        }
    }
}

/// Maps the vendor response; any unexpected shape becomes `unknown`.
#[must_use]
pub fn parse_storage_status(value: &Value) -> StorageReport {
    if value.get("resultCode").and_then(integer) != Some(0) {
        return StorageReport::unknown();
    }
    let Some(status) = value.get("storageStatus").filter(|item| item.is_object()) else {
        return StorageReport::unknown();
    };
    let cards = status
        .get("storageList")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    // Cameras expose one card; prefer the lowest index when several appear.
    let Some(card) = cards
        .iter()
        .filter(|card| card.is_object())
        .min_by_key(|card| card.get("index").and_then(integer).unwrap_or(i64::MAX))
    else {
        return StorageReport {
            status: StorageState::NoCard,
            capacity_mb: None,
            free_mb: None,
        };
    };
    let state = match card.get("status").and_then(integer) {
        Some(0) => StorageState::Ok,
        Some(1) => StorageState::Error,
        Some(2) => StorageState::Unformatted,
        _ => StorageState::Unknown,
    };
    let capacity_mb = card
        .get("capacity")
        .and_then(integer)
        .and_then(|value| u64::try_from(value).ok())
        .filter(|value| *value > 0);
    StorageReport {
        status: state,
        capacity_mb,
        free_mb: None,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn report(value: &Value) -> Value {
        serde_json::to_value(parse_storage_status(value)).unwrap_or_else(|error| panic!("{error}"))
    }

    #[test]
    fn healthy_card_reports_capacity_in_mib() {
        let value = json!({"resultCode": "0", "storageStatus": {"result": "0",
            "formatingRate": "", "storageList": [
                {"index": 1, "type": 0, "status": 0, "capacity": 61120,
                 "firstRecordTime": "2026-09-01 08:00:00"}]}});
        assert_eq!(
            report(&value),
            json!({"status": "ok", "capacity_mb": 61120})
        );
    }

    #[test]
    fn card_states_follow_the_app_adapter() {
        for (code, expected) in [
            (1, "error"),
            (2, "unformatted"),
            (3, "unknown"),
            (9, "unknown"),
        ] {
            let value = json!({"resultCode": 0, "storageStatus": {"storageList": [
                {"index": 1, "status": code, "capacity": "0"}]}});
            assert_eq!(report(&value), json!({"status": expected}), "{code}");
        }
    }

    #[test]
    fn missing_list_means_no_card_and_errors_mean_unknown() {
        let empty = json!({"resultCode": 0, "storageStatus": {"storageList": []}});
        assert_eq!(report(&empty), json!({"status": "no_card"}));
        let absent = json!({"resultCode": 0, "storageStatus": {"result": "0"}});
        assert_eq!(report(&absent), json!({"status": "no_card"}));
        for value in [
            json!({"resultCode": "-1"}),
            json!({"resultCode": 0}),
            json!({"resultCode": 0, "storageStatus": null}),
            json!({}),
        ] {
            assert_eq!(report(&value), json!({"status": "unknown"}));
        }
    }

    #[test]
    fn lowest_index_wins_when_several_cards_exist() {
        let value = json!({"resultCode": 0, "storageStatus": {"storageList": [
            {"index": 2, "status": 2}, {"index": 1, "status": 0, "capacity": 100}]}});
        assert_eq!(report(&value), json!({"status": "ok", "capacity_mb": 100}));
    }
}
