//! Envelopes written by vistoda-blink 0.25.1 (`credentials.rs` at
//! 2da5ddf4e1cf) must keep opening byte-for-byte: `blink-provider-v2.sealed`
//! is sealed with the `store.key` sub-key, `blink-provider-v1.sealed` with the
//! older token-derived key. The key file used for the fixture is bytes 0..32.
#![allow(clippy::expect_used)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use vistoda_provider_kit::KeyLabel;
use vistoda_provider_kit::sealed_store::{
    KEY_FILE_NAME, KeyFile, SealError, SealedFile, SealingKey, open,
};

const BLINK_STORE: KeyLabel = KeyLabel::new("vistoda-blink/provider-store/v1");
const BLINK_LEGACY_LABEL: &str = "vistoda-blink-provider-store-v1";

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    fs::read(path).expect("fixture")
}

fn install(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join("provider.sealed");
    fs::write(&path, fixture(name)).expect("write store");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
    let key = directory.join(KEY_FILE_NAME);
    fs::write(&key, (0_u8..32).collect::<Vec<_>>()).expect("write key");
    fs::set_permissions(&key, fs::Permissions::from_mode(0o600)).expect("chmod");
    path
}

fn blink_store(path: PathBuf, workload_token: &str) -> SealedFile {
    let legacy = SealingKey::from_label_and_material(BLINK_LEGACY_LABEL, workload_token.as_bytes());
    let key = KeyFile::beside(&path, BLINK_STORE);
    SealedFile::new(path, key).with_legacy_key(legacy)
}

#[test]
fn blink_store_key_envelopes_open() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = install(directory.path(), "blink-provider-v2.sealed");
    let store = blink_store(path, &"z".repeat(64));
    let plaintext = store.load().expect("load").expect("present");
    assert_eq!(*plaintext, fixture("blink-provider.json"));
    let key = KeyFile::beside(store.path(), BLINK_STORE)
        .existing()
        .expect("key");
    let (version, opened) = open(&key, &fixture("blink-provider-v2.sealed")).expect("open");
    assert_eq!(version, 2);
    assert_eq!(*opened, fixture("blink-provider.json"));
}

#[test]
fn blink_legacy_token_envelopes_open_and_are_resealed() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = install(directory.path(), "blink-provider-v1.sealed");
    assert!(matches!(
        blink_store(path.clone(), &"d".repeat(64)).load(),
        Err(SealError::Authentication)
    ));
    let store = blink_store(path.clone(), &"c".repeat(64));
    let plaintext = store.load().expect("load").expect("present");
    assert_eq!(*plaintext, fixture("blink-provider.json"));
    let resealed: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).expect("read")).expect("json");
    assert_eq!(resealed["version"], 2);
    let reopened = blink_store(path, &"d".repeat(64));
    assert_eq!(
        *reopened.load().expect("load").expect("present"),
        fixture("blink-provider.json")
    );
}
