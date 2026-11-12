#![allow(clippy::expect_used)]

use std::fs;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::Path;

use vistoda_provider_kit::private_file::{
    self, FileError, FileOwner, PrivateFileError, ReadPolicy, create_new, read, read_optional,
    read_string, write_atomic, write_json,
};

fn mode(path: &Path) -> u32 {
    fs::metadata(path).expect("metadata").permissions().mode() & 0o777
}

fn write_with_mode(path: &Path, bytes: &[u8], mode: u32) {
    fs::write(path, bytes).expect("write");
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).expect("chmod");
}

#[test]
fn atomic_writes_are_private_complete_and_need_a_parent() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("state/value.json");
    let value = serde_json::json!({"complete": true});
    assert!(
        write_json(&path, &value)
            .expect_err("no parent")
            .is_not_found()
    );
    private_file::create_parent(&path).expect("parent");
    write_json(&path, &value).expect("write json");
    assert_eq!(
        fs::read_to_string(&path).expect("read"),
        "{\"complete\":true}\n"
    );
    assert_eq!(mode(&path), 0o600);
    assert_eq!(mode(&directory.path().join("state")), 0o700);
    write_atomic(&path, b"replaced").expect("replace");
    assert_eq!(fs::read(&path).expect("read"), b"replaced");
    let leftovers = fs::read_dir(directory.path().join("state"))
        .expect("list")
        .count();
    assert_eq!(leftovers, 1, "no temporary file may remain");
}

#[test]
fn an_existing_directory_keeps_its_mode() {
    let directory = tempfile::tempdir().expect("temporary directory");
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o750)).expect("chmod");
    let nested = directory.path().join("receipt");
    private_file::create_parent(&nested).expect("existing parent");
    write_atomic(&nested, b"x").expect("write");
    assert_eq!(mode(directory.path()), 0o750);
}

#[test]
fn reads_refuse_broad_modes_symlinks_short_and_empty_files() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("token");
    let policy = ReadPolicy::new(64).min_bytes(32);
    write_with_mode(&path, &[b'x'; 32], 0o644);
    assert!(matches!(
        read(&path, policy),
        Err(FileError::Private(
            PrivateFileError::InsecurePermissions { .. }
        ))
    ));
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).expect("chmod");
    assert_eq!(read(&path, policy).expect("private read").len(), 32);
    let link = directory.path().join("link");
    symlink(&path, &link).expect("symlink");
    assert!(matches!(
        read(&link, policy),
        Err(FileError::Private(PrivateFileError::Symlink))
    ));
    write_with_mode(&path, b"short", 0o600);
    assert!(matches!(
        read(&path, policy),
        Err(FileError::TooShort { minimum: 32 })
    ));
    write_with_mode(&path, b"", 0o600);
    assert!(matches!(
        read(&path, policy),
        Err(FileError::Private(PrivateFileError::Empty))
    ));
    write_with_mode(&path, &[b'x'; 65], 0o600);
    assert!(matches!(
        read(&path, policy),
        Err(FileError::Private(PrivateFileError::TooLarge { .. }))
    ));
}

#[test]
fn trimmed_strings_and_absent_files() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("token");
    write_with_mode(&path, b"  0123456789abcdef0123456789abcdef\n", 0o600);
    let policy = ReadPolicy::new(128)
        .min_bytes(32)
        .trimmed()
        .owner(FileOwner::Any);
    assert_eq!(
        read_string(&path, policy).expect("token").as_str(),
        "0123456789abcdef0123456789abcdef"
    );
    let missing = directory.path().join("missing");
    assert!(read_optional(&missing, policy).expect("absent").is_none());
    assert!(read(&missing, policy).expect_err("missing").is_not_found());
}

#[test]
fn create_new_never_replaces_an_existing_file() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let path = directory.path().join("store.key");
    assert!(create_new(&path, b"first").expect("create"));
    assert!(!create_new(&path, b"second").expect("second create"));
    assert_eq!(fs::read(&path).expect("read"), b"first");
    assert_eq!(mode(&path), 0o600);
    assert_eq!(fs::read_dir(directory.path()).expect("list").count(), 1);
    private_file::remove(&path).expect("remove");
    private_file::remove(&path).expect("absent is fine");
    assert!(!path.exists());
}
