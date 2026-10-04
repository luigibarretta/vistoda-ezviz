//! Verification-code selection: option first, cloud only when needed.

use std::{os::unix::fs::PermissionsExt, sync::atomic::Ordering};

use super::tests::{CODE, Fake, camera};
use super::*;

fn option_file(directory: &std::path::Path) -> std::path::PathBuf {
    let path = directory.join("front.code");
    std::fs::write(&path, format!("{CODE}\n")).unwrap_or_else(|e| panic!("{e}"));
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
        .unwrap_or_else(|e| panic!("{e}"));
    path
}

fn hashed(code: &str) -> DeviceStatus {
    DeviceStatus {
        encrypt_pwd_hash: Some(twice_md5(code)),
        ..DeviceStatus::default()
    }
}

fn sources(keys: &[VideoKey]) -> Vec<KeySource> {
    keys.iter().map(|key| key.source).collect()
}

#[tokio::test]
async fn usable_option_code_never_contacts_the_cloud() {
    let directory = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let with_option = camera(Some(option_file(directory.path())));
    let fake = Fake {
        cloud: Some(CODE),
        ..Fake::default()
    };
    for status in [Some(hashed(CODE)), None] {
        let keys = candidate_keys(&fake, &with_option, status.as_ref()).await;
        assert_eq!(sources(&keys), [KeySource::Option]);
        assert_eq!(keys[0].code(), CODE);
    }
    assert_eq!(fake.cloud_calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cloud_code_is_requested_only_without_a_usable_option() {
    let directory = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let with_option = camera(Some(option_file(directory.path())));
    let fake = Fake {
        cloud: Some("ZZZZZZ"),
        ..Fake::default()
    };
    // A stale option code yields the matching cloud copy.
    let keys = candidate_keys(&fake, &with_option, Some(&hashed("ZZZZZZ"))).await;
    assert_eq!(sources(&keys), [KeySource::Cloud]);
    assert_eq!(fake.cloud_calls.load(Ordering::SeqCst), 1);
    // A cloud copy that does not match the hash is rejected.
    let keys = candidate_keys(&fake, &camera(None), Some(&hashed(CODE))).await;
    assert!(keys.is_empty());
    let offline = Fake::default();
    assert!(
        candidate_keys(&offline, &camera(None), None)
            .await
            .is_empty()
    );
}

#[test]
fn reported_key_source_uses_only_local_state() {
    let directory = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
    let with_option = camera(Some(option_file(directory.path())));
    let fake = Fake {
        cloud: Some(CODE),
        ..Fake::default()
    };
    let status = hashed(CODE);
    assert_eq!(
        report_key_source(&fake, &with_option, Some(&status)),
        KeySource::Option
    );
    assert_eq!(
        report_key_source(&fake, &camera(None), Some(&status)),
        KeySource::None
    );
    let cached = Fake {
        cached: Some(CODE),
        ..Fake::default()
    };
    assert_eq!(
        report_key_source(&cached, &camera(None), Some(&status)),
        KeySource::Cloud
    );
    assert_eq!(fake.cloud_calls.load(Ordering::SeqCst), 0);
}
