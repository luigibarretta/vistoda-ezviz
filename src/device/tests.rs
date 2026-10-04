//! Synthetic fixtures for record decoding, key resolution and service caches.

use std::{
    collections::BTreeMap,
    os::unix::fs::PermissionsExt,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::*;

pub(super) const CODE: &str = "ABCDEF";

pub(super) fn packed(bytes: &[u8]) -> String {
    STANDARD.encode(miniz_oxide::deflate::compress_to_vec_zlib(bytes, 6))
}

pub(super) fn utc_status() -> DeviceStatus {
    DeviceStatus {
        utc_offset_seconds: Some(0),
        ..DeviceStatus::default()
    }
}

fn camera(key_file: Option<std::path::PathBuf>) -> CameraConfig {
    CameraConfig {
        serial: "never-exposed".into(),
        channel: 1,
        substream: false,
        decrypt_video: false,
        media_key_file: key_file,
    }
}

#[derive(Default)]
pub(super) struct Fake {
    pub(super) status: Option<DeviceStatus>,
    pub(super) cloud: Option<&'static str>,
    pub(super) pages: Vec<Value>,
    pub(super) storage_calls: AtomicUsize,
    pub(super) record_calls: AtomicUsize,
}

#[async_trait]
impl DeviceSource for Fake {
    async fn device_status(&self, _: &CameraConfig) -> Result<DeviceStatus, BridgeError> {
        self.status.clone().ok_or(BridgeError::UpstreamUnavailable)
    }

    async fn cloud_verification_code(
        &self,
        _: &CameraConfig,
    ) -> Result<Zeroizing<String>, BridgeError> {
        self.cloud
            .map(|code| Zeroizing::new(code.to_owned()))
            .ok_or(BridgeError::UpstreamUnavailable)
    }

    async fn storage_status(&self, _: &CameraConfig) -> Result<Value, BridgeError> {
        self.storage_calls.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"resultCode": 0, "storageStatus": {"storageList": [
            {"index": 1, "status": 0, "capacity": 30_000}]}}))
    }

    async fn record_page(
        &self,
        _: &CameraConfig,
        query: &RecordQuery,
    ) -> Result<Value, BridgeError> {
        let index = self.record_calls.fetch_add(1, Ordering::SeqCst);
        if query.version == RecordSearchVersion::Common {
            return Err(BridgeError::Upstream("common unsupported".into()));
        }
        Ok(self
            .pages
            .get(index)
            .cloned()
            .unwrap_or_else(|| json!({"meta": {"code": 200}})))
    }
}

pub(super) fn service(fake: Fake) -> (Arc<DeviceService>, Arc<Fake>) {
    let cameras = BTreeMap::from([("front".to_owned(), camera(None))]);
    let service = DeviceService::new(&cameras);
    let fake = Arc::new(fake);
    service.attach(Arc::clone(&fake) as Arc<dyn DeviceSource>);
    (service, fake)
}

pub(super) fn v2_page(items: &Value, finished: i64) -> Value {
    json!({"meta": {"code": 200}, "isFinished": finished,
           "records": packed(items.to_string().as_bytes())})
}

#[test]
fn v2_payload_decodes_local_times_and_types() {
    let items = json!([
        {"B": "2026-10-04T08:00:00", "E": "2026-10-04T08:00:30", "Type": 1, "Res": "", "Res2": ""},
        {"B": "2026-10-04T09:00:00", "E": "2026-10-04T09:30:00", "Type": 0},
        {"B": "2026-10-04T10:00:00", "E": "2026-10-04T10:00:00", "Type": 0},
        {"B": "bad", "E": "2026-10-04T11:00:00"}
    ]);
    let status = DeviceStatus {
        utc_offset_seconds: Some(7200),
        ..DeviceStatus::default()
    };
    let page = parse_v2_page(&v2_page(&items, 0), &status).unwrap_or_else(|e| panic!("{e}"));
    assert!(!page.finished);
    assert_eq!(page.last_end.as_deref(), Some("2026-10-04T10:00:00"));
    let midnight_utc = 1_791_072_000 - 7200;
    assert_eq!(
        serde_json::to_value(&page.records).unwrap_or_default(),
        json!([
            {"start": midnight_utc + 8 * 3600, "end": midnight_utc + 8 * 3600 + 30, "type": "event"},
            {"start": midnight_utc + 9 * 3600, "end": midnight_utc + 9 * 3600 + 1800, "type": "continuous"}
        ])
    );
}

#[test]
fn common_payload_decodes_packed_entries() {
    let data = [
        8, 0, 0, 8, 0, 30, 1, 0, // event 08:00:00-08:00:30
        9, 0, 0, 9, 30, 0, 0, 0, // continuous
        25, 0, 0, 26, 0, 0, 0, 0, // invalid hour
        12, 0, 0, 11, 0, 0, 9, 0, // reversed
        13, 0, 0, 13, 1, 0, 9, 0, // other
    ];
    let value = json!({"meta": {"code": 200}, "baseDay": "2026-10-04", "searchCount": 5,
                       "isFinished": 1, "data": packed(&data)});
    let records = parse_common(&value, &utc_status()).unwrap_or_else(|e| panic!("{e}"));
    let kinds: Vec<_> = records.iter().map(|record| record.kind.as_str()).collect();
    assert_eq!(kinds, ["event", "continuous", "other"]);
    assert_eq!(records[0].start, 1_791_072_000 + 8 * 3600);
    assert_eq!(records[0].end - records[0].start, 30);
    let empty = json!({"meta": {"code": 200}, "baseDay": "", "data": ""});
    assert!(
        parse_common(&empty, &utc_status())
            .unwrap_or_default()
            .is_empty()
    );
    let refused = json!({"meta": {"code": 2009}});
    assert!(parse_common(&refused, &utc_status()).is_err());
}

#[test]
fn payload_decoding_is_bounded_and_strict() {
    assert!(decode_payload("not base64!").is_err());
    assert!(decode_payload(&STANDARD.encode(b"not zlib")).is_err());
    let bomb = packed(&vec![0_u8; 2 * 1024 * 1024]);
    assert!(decode_payload(&bomb).is_err());
    assert_eq!(decode_payload(&packed(b"[]")).unwrap_or_default(), b"[]");
}

#[test]
fn normalize_sorts_dedups_and_caps() {
    let record = |start| SdRecord {
        start,
        end: start + 1,
        kind: "continuous".into(),
    };
    let mut input: Vec<_> = (0..600).rev().map(record).collect();
    input.push(record(5));
    let output = normalize(input);
    assert_eq!(output.len(), MAX_RECORDS);
    assert_eq!(output[0].start, 0);
    assert_eq!(output[5].start, 5);
    assert_eq!(output[6].start, 6);
}

#[tokio::test]
async fn option_code_wins_only_when_it_matches_the_cloud_hash() {
    let directory = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let path = directory.path().join("front.code");
    std::fs::write(&path, format!("{CODE}\n")).unwrap_or_else(|e| panic!("{e}"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|e| panic!("{e}"));
    let matching = DeviceStatus {
        encrypt_pwd_hash: Some(twice_md5(CODE)),
        ..DeviceStatus::default()
    };
    let fake = Fake {
        cloud: Some(CODE),
        ..Fake::default()
    };
    let with_option = camera(Some(path.clone()));
    let key = resolve_video_key(&fake, &with_option, Some(&matching)).await;
    assert_eq!(key.as_ref().map(|key| key.source), Some(KeySource::Option));
    assert_eq!(key.as_ref().map(VideoKey::code), Some(CODE));

    let other = DeviceStatus {
        encrypt_pwd_hash: Some(twice_md5("ZZZZZZ")),
        ..DeviceStatus::default()
    };
    assert!(
        resolve_video_key(&fake, &with_option, Some(&other))
            .await
            .is_none()
    );
    let cloud = resolve_video_key(&fake, &camera(None), Some(&matching)).await;
    assert_eq!(cloud.map(|key| key.source), Some(KeySource::Cloud));
    let offline = Fake::default();
    assert!(
        resolve_video_key(&offline, &camera(None), None)
            .await
            .is_none()
    );
    // Without a hash both sources are candidates, option first.
    let cloud_other = Fake {
        cloud: Some("ZZZZZZ"),
        ..Fake::default()
    };
    let both = candidate_keys(&cloud_other, &with_option, None).await;
    let sources: Vec<_> = both.iter().map(|key| key.source).collect();
    assert_eq!(sources, [KeySource::Option, KeySource::Cloud]);
    // With a hash, a stale option code yields only the matching cloud copy.
    assert!(
        candidate_keys(&fake, &with_option, Some(&other))
            .await
            .is_empty()
    );
    let stale = DeviceStatus {
        encrypt_pwd_hash: Some(twice_md5("ZZZZZZ")),
        ..DeviceStatus::default()
    };
    let fallback = candidate_keys(&cloud_other, &with_option, Some(&stale)).await;
    let sources: Vec<_> = fallback.iter().map(|key| key.source).collect();
    assert_eq!(sources, [KeySource::Cloud]);
}
