//! Service cache, key-source and record-search behavior with a fake source.

use std::sync::atomic::Ordering;

use chrono::NaiveDate;
use serde_json::json;
use tokio::time::Instant;

use super::tests::{CODE, Fake, service, utc_status, v2_page};
use super::*;

#[tokio::test]
async fn storage_is_cached_until_its_window_expires() {
    let (cached, fake) = service(Fake::default());
    for _ in 0..3 {
        let report = cached
            .storage("front")
            .await
            .unwrap_or_else(|e| panic!("{e}"));
        assert_eq!(report.status, StorageState::Ok);
    }
    assert_eq!(fake.storage_calls.load(Ordering::SeqCst), 1);
    {
        let slot = cached.slot("front").unwrap_or_else(|e| panic!("{e}"));
        let mut cached = slot.storage.lock().await;
        let entry = cached
            .as_mut()
            .unwrap_or_else(|| panic!("storage was not cached"));
        assert!(entry.expires >= Instant::now() + STORAGE_TTL - FAILURE_TTL);
        entry.expires = Instant::now();
    }
    cached
        .storage("front")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(fake.storage_calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        cached.storage("rear").await,
        Err(BridgeError::CameraNotFound)
    ));
}

#[tokio::test]
async fn encryption_report_combines_status_and_key_source() {
    let (known, _) = service(Fake {
        status: Some(DeviceStatus {
            video_encrypted: Some(true),
            encrypt_pwd_hash: Some(twice_md5(CODE)),
            ..DeviceStatus::default()
        }),
        cloud: Some(CODE),
        ..Fake::default()
    });
    let report = known
        .encryption("front")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        serde_json::to_value(report).unwrap_or_default(),
        json!({"video_encrypted": true, "key_source": "cloud"})
    );
    let (unknown, _) = service(Fake::default());
    let report = unknown
        .encryption("front")
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        serde_json::to_value(report).unwrap_or_default(),
        json!({"video_encrypted": null, "key_source": "none"})
    );
}

#[tokio::test]
async fn v2_search_follows_continuation_and_stops_without_progress() {
    let first = json!([{"B": "2026-10-04T01:00:00", "E": "2026-10-04T02:00:00", "Type": 0}]);
    let second = json!([{"B": "2026-10-04T02:00:00", "E": "2026-10-04T03:00:00", "Type": 1}]);
    let (records, fake) = service(Fake {
        status: Some(utc_status()),
        pages: vec![v2_page(&first, 0), v2_page(&second, 0), v2_page(&second, 0)],
        ..Fake::default()
    });
    let day = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap_or_default();
    let list = records
        .sd_records("front", day)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(list.records.len(), 2);
    assert_eq!(list.records[1].kind, "event");
    // Third page repeats the cursor, so the search stops there.
    assert_eq!(fake.record_calls.load(Ordering::SeqCst), 3);
    // The same day is served from the per-camera cache.
    let again = records
        .sd_records("front", day)
        .await
        .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(again, list);
    assert_eq!(fake.record_calls.load(Ordering::SeqCst), 3);
}

#[test]
fn record_cache_is_short_for_recent_days() {
    let today = NaiveDate::from_ymd_opt(2026, 10, 4).unwrap_or_default();
    let yesterday = NaiveDate::from_ymd_opt(2026, 10, 3).unwrap_or_default();
    let older = NaiveDate::from_ymd_opt(2026, 10, 1).unwrap_or_default();
    assert_eq!(service::record_ttl(today, today), RECORDS_RECENT_TTL);
    assert_eq!(service::record_ttl(yesterday, today), RECORDS_RECENT_TTL);
    assert_eq!(service::record_ttl(older, today), RECORDS_PAST_TTL);
}
