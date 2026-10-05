mod controls_fake;
mod support;

use axum::http::StatusCode;
use ezviz_vtm_bridge::controls::VendorWrite;
use serde_json::json;

use controls_fake::{attached, call};
use support::json;

const PATH: &str = "/v1/cameras/front/controls";

#[tokio::test]
async fn malformed_or_unknown_writes_are_rejected_without_vendor_calls() {
    let (system, fake, _) = attached();
    for body in [
        json!({}),
        json!({"key": "switch.privacy", "value": true}),
        json!({"key": "switch.privacy", "value": true, "expected_value": false, "extra": 1}),
        json!({"key": "switch.remote_unlock", "value": true, "expected_value": false}),
        json!({"key": "switch.privacy", "value": 1, "expected_value": false}),
        json!({"key": "detection_mode", "value": "motion", "expected_value": "human_shape"}),
        json!({"key": "sensitivity", "value": "10", "expected_value": 50}),
        json!({"key": "online", "value": true, "expected_value": true}),
    ] {
        let response = call(&system, "PUT", PATH, &body).await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{body}");
        assert_eq!(json(response).await["error"], "invalid_control");
    }
    let range = json!({"key": "sensitivity", "value": 101, "expected_value": 50});
    assert_eq!(
        call(&system, "PUT", PATH, &range).await.status(),
        StatusCode::BAD_REQUEST
    );
    assert!(fake.with(|state| state.writes.is_empty()));
}

#[tokio::test]
async fn stale_expected_values_conflict_and_missing_controls_are_unsupported() {
    let (system, fake, _) = attached();
    let stale = json!({"key": "switch.sleep", "value": false, "expected_value": false});
    let conflict = call(&system, "PUT", PATH, &stale).await;
    assert_eq!(conflict.status(), StatusCode::CONFLICT);
    assert_eq!(
        json(conflict).await,
        json!({"error": "conflict", "current": true})
    );
    // Type 10 is reported only with supportExt "48", which this camera lacks.
    let absent = json!({"key": "switch.infrared_light", "value": true, "expected_value": false});
    let unsupported = call(&system, "PUT", PATH, &absent).await;
    assert_eq!(unsupported.status(), StatusCode::CONFLICT);
    assert_eq!(
        json(unsupported).await,
        json!({"error": "unsupported_control"})
    );
    assert!(fake.with(|state| state.writes.is_empty()));
}

#[tokio::test(start_paused = true)]
async fn confirmed_writes_return_the_read_back_controls() {
    let (system, fake, _) = attached();
    let cases = [
        (
            json!({"key": "switch.privacy", "value": true, "expected_value": false}),
            VendorWrite::Switch {
                kind: 7,
                enable: true,
            },
        ),
        (
            json!({"key": "defence_enabled", "value": false, "expected_value": true}),
            VendorWrite::Defence(false),
        ),
        (
            json!({"key": "detection_mode", "value": "pir", "expected_value": "human_shape"}),
            VendorWrite::DetectionMode(5),
        ),
        (
            json!({"key": "sensitivity", "value": 80, "expected_value": 50}),
            VendorWrite::Sensitivity { kind: 3, value: 80 },
        ),
    ];
    for (body, expected) in cases {
        let response = call(&system, "PUT", PATH, &body).await;
        assert_eq!(response.status(), StatusCode::OK, "{body}");
        let controls = json(response).await;
        assert_eq!(
            fake.with(|state| state.writes.last().copied()),
            Some(expected)
        );
        let key = body["key"].as_str().unwrap_or_default();
        let value = key.strip_prefix("switch.").map_or_else(
            || {
                if key == "sensitivity" {
                    controls["sensitivity"]["value"].clone()
                } else {
                    controls[key].clone()
                }
            },
            |name| controls["switches"][name].clone(),
        );
        assert_eq!(value, body["value"], "{body}");
    }
    assert_eq!(fake.with(|state| state.writes.len()), 4);
    // Writing the current value is a confirmed no-op without a vendor call.
    let same = json!({"key": "switch.privacy", "value": true, "expected_value": true});
    assert_eq!(
        call(&system, "PUT", PATH, &same).await.status(),
        StatusCode::OK
    );
    assert_eq!(fake.with(|state| state.writes.len()), 4);
}

#[tokio::test(start_paused = true)]
async fn unconfirmed_writes_roll_back_once_and_report_bad_gateway() {
    let (system, fake, _) = attached();
    fake.with(|state| state.sticky = true);
    let body = json!({"key": "switch.status_light", "value": false, "expected_value": true});
    let response = call(&system, "PUT", PATH, &body).await;
    assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(json(response).await, json!({"error": "unconfirmed"}));
    assert_eq!(
        fake.with(|state| state.writes.clone()),
        [
            VendorWrite::Switch {
                kind: 3,
                enable: false
            },
            VendorWrite::Switch {
                kind: 3,
                enable: true
            }
        ]
    );
    // Before-read plus three bounded read-backs, then the cache is dropped.
    assert_eq!(fake.with(|state| state.pages), 4);
    let reread = call(&system, "GET", PATH, &serde_json::Value::Null).await;
    assert_eq!(reread.status(), StatusCode::OK);
    assert_eq!(fake.with(|state| state.pages), 5);
}
