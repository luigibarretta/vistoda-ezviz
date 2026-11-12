#![allow(clippy::expect_used)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};

use vistoda_provider_kit::KeyLabel;
use vistoda_provider_kit::private_file::{FileError, PrivateFileError};
use vistoda_provider_kit::sealed_store::{
    KEY_FILE_NAME, KeyFile, Migration, SealError, SealedFile, SealingKey, VERSION_LEGACY_KEY, seal,
};

const SESSION: KeyLabel = KeyLabel::new("vistoda-test/session/v1");
const PUSH: KeyLabel = KeyLabel::new("vistoda-test/push/v1");
const SECRET: &[u8] = br#"{"refresh_token":"sensitive-refresh-token"}"#;

fn sealed(directory: &Path, label: KeyLabel) -> SealedFile {
    let path = directory.join("state.sealed");
    SealedFile::new(path.clone(), KeyFile::beside(&path, label))
}

fn write_private(path: &Path, bytes: &[u8]) {
    fs::write(path, bytes).expect("write");
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).expect("chmod");
}

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("metadata").permissions().mode() & 0o777
}

fn envelope(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).expect("read")).expect("envelope")
}

#[test]
fn round_trip_is_private_and_encrypted() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let file = sealed(directory.path(), SESSION);
    assert!(file.load().expect("empty load").is_none());
    assert!(!file.exists().expect("exists"));
    file.save(SECRET).expect("save");
    let raw = fs::read_to_string(file.path()).expect("read");
    assert!(!raw.contains("sensitive"));
    assert_eq!(envelope(file.path())["version"], 2);
    assert_eq!(mode(file.path()), 0o600);
    let key = directory.path().join(KEY_FILE_NAME);
    assert_eq!(mode(&key), 0o600);
    assert_eq!(fs::metadata(&key).expect("key").len(), 32);
    let reopened = sealed(directory.path(), SESSION);
    assert_eq!(&*reopened.load().expect("load").expect("present"), SECRET);
    assert!(reopened.exists().expect("exists"));
}

#[test]
fn tampering_and_other_keys_fail_authentication() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let file = sealed(directory.path(), SESSION);
    file.save(SECRET).expect("save");
    // Another label of the same key file is a different key.
    let other = sealed(directory.path(), PUSH);
    assert!(matches!(other.load(), Err(SealError::Authentication)));
    let mut value = envelope(file.path());
    let ciphertext = value["ciphertext"].as_str().expect("ciphertext").to_owned();
    let flipped = if ciphertext.starts_with('A') {
        "B"
    } else {
        "A"
    };
    value["ciphertext"] = format!("{flipped}{}", &ciphertext[1..]).into();
    write_private(file.path(), &serde_json::to_vec(&value).expect("json"));
    assert!(matches!(
        sealed(directory.path(), SESSION).load(),
        Err(SealError::Authentication)
    ));
    write_private(file.path(), b"not json");
    assert!(matches!(
        sealed(directory.path(), SESSION).load(),
        Err(SealError::Encoding)
    ));
    write_private(file.path(), br#"{"version":3,"nonce":"","ciphertext":""}"#);
    assert!(matches!(
        sealed(directory.path(), SESSION).load(),
        Err(SealError::UnsupportedVersion(3))
    ));
}

#[test]
fn a_lost_or_replaced_key_fails_closed() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let file = sealed(directory.path(), SESSION);
    file.save(SECRET).expect("save");
    let key = directory.path().join(KEY_FILE_NAME);
    fs::remove_file(&key).expect("remove key");
    let reopened = sealed(directory.path(), SESSION);
    assert!(matches!(reopened.load(), Err(SealError::File(error)) if error.is_not_found()));
    assert!(!key.exists(), "opening must never create key material");
    write_private(&key, &(0_u8..32).collect::<Vec<_>>());
    assert!(matches!(
        sealed(directory.path(), SESSION).load(),
        Err(SealError::Authentication)
    ));
    write_private(&key, &[7_u8; 32]);
    assert!(matches!(
        sealed(directory.path(), SESSION).load(),
        Err(SealError::Key(_))
    ));
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(matches!(
        sealed(directory.path(), SESSION).load(),
        Err(SealError::File(FileError::Private(
            PrivateFileError::InsecurePermissions { .. }
        )))
    ));
}

#[test]
fn symlinked_or_shared_sealed_files_are_refused() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let file = sealed(directory.path(), SESSION);
    file.save(SECRET).expect("save");
    fs::set_permissions(file.path(), fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(matches!(file.load(), Err(SealError::File(_))));
    let target = directory.path().join("elsewhere");
    fs::rename(file.path(), &target).expect("move");
    symlink(&target, file.path()).expect("symlink");
    assert!(matches!(
        file.load(),
        Err(SealError::File(FileError::Private(
            PrivateFileError::Symlink
        )))
    ));
}

#[test]
fn legacy_envelopes_open_with_the_legacy_key_and_are_resealed() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state.sealed");
    let legacy = SealingKey::from_label_and_material("vistoda-test-v1", b"token");
    write_private(
        &path,
        &seal(&legacy, VERSION_LEGACY_KEY, SECRET).expect("seal"),
    );
    let without = SealedFile::new(path.clone(), KeyFile::beside(&path, SESSION));
    assert!(matches!(
        without.load(),
        Err(SealError::UnsupportedVersion(1))
    ));
    let with = without.clone().with_legacy_key(legacy);
    assert_eq!(&*with.load().expect("legacy").expect("present"), SECRET);
    assert_eq!(envelope(&path)["version"], 2);
    assert_eq!(
        &*without.load().expect("resealed").expect("present"),
        SECRET
    );
}

fn migrating(directory: &Path) -> (SealedFile, PathBuf) {
    let plaintext = directory.join("state.json");
    (
        sealed(directory, SESSION).migrating_plaintext(plaintext.clone()),
        plaintext,
    )
}

#[test]
fn plaintext_is_sealed_once_and_then_removed() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (file, plaintext) = migrating(directory.path());
    assert_eq!(file.migrate().expect("nothing to do"), Migration::Nothing);
    write_private(&plaintext, SECRET);
    assert!(file.exists().expect("exists after migration"));
    assert!(!plaintext.exists());
    assert_eq!(&*file.load().expect("load").expect("present"), SECRET);
    assert!(
        !fs::read_to_string(file.path())
            .expect("sealed")
            .contains("sensitive")
    );
}

#[test]
fn a_rejected_plaintext_file_is_kept_and_nothing_is_sealed() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (file, plaintext) = migrating(directory.path());
    fs::write(&plaintext, SECRET).expect("write");
    fs::set_permissions(&plaintext, fs::Permissions::from_mode(0o644)).expect("chmod");
    assert!(matches!(file.load(), Err(SealError::File(_))));
    assert!(plaintext.exists());
    assert!(!file.path().exists());
}

#[test]
fn stale_plaintext_is_removed_only_when_the_sealed_copy_opens() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let (file, plaintext) = migrating(directory.path());
    file.save(b"sealed wins").expect("save");
    write_private(&plaintext, SECRET);
    assert_eq!(file.migrate().expect("stale"), Migration::StaleRemoved);
    assert!(!plaintext.exists());
    assert_eq!(
        &*file.load().expect("load").expect("present"),
        b"sealed wins"
    );
    write_private(&plaintext, SECRET);
    write_private(file.path(), b"corrupt");
    assert!(matches!(file.migrate(), Err(SealError::Encoding)));
    assert!(plaintext.exists(), "never drop the only readable copy");
}
