//! Synthetic fixtures for message parsing, categories and picture decoding.

use aes::Aes128;
use cbc::Encryptor;
use cipher::{BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest as _, Md5};
use serde_json::json;

use super::{PictureError, category, decode_picture, model, parse_list, parse_summary};

mod feed;
mod poller;

pub(super) const JPEG: &[u8] = &[
    0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, b'J', b'F', b'I', b'F', 0, 1, 2, 3, 0xff, 0xd9,
];
const CHECKSUM: &str = "00112233445566778899aabbccddeeff";
const IV: [u8; 16] = *b"01234567\0\0\0\0\0\0\0\0";

fn encrypt(key: &[u8; 16], clear: &[u8]) -> Vec<u8> {
    let mut buffer = clear.to_vec();
    buffer.resize(clear.len() + 16, 0);
    let length = Encryptor::<Aes128>::new(key.into(), &IV.into())
        .encrypt_padded_mut::<Pkcs7>(&mut buffer, clear.len())
        .unwrap_or_else(|error| panic!("{error}"))
        .len();
    buffer.truncate(length);
    buffer
}

fn checksum_key() -> [u8; 16] {
    let mut key = [0_u8; 16];
    key.copy_from_slice(&hex::decode(CHECKSUM).unwrap_or_default());
    key
}

/// A 48-byte header (16-byte magic plus 32 opaque bytes) and ciphertext.
fn headed_block(key: &[u8; 16], clear: &[u8]) -> Vec<u8> {
    let mut block = b"hikencodepicture".to_vec();
    block.extend_from_slice(&[b'0'; 32]);
    block.extend(encrypt(key, clear));
    block
}

#[test]
fn summary_fixture_yields_top_message_total_and_offset() {
    let value = json!({"meta":{"code":200}, "summaries":[
        {"deviceSerial":"CAM1","total":"7","unread":2,"topMessage":{
            "msgId":"m-7","time":1_700_000_000_000_i64,"timeStr":"2023-11-14 23:13:20"}},
        {"deviceSerial":"CAM2","total":0},
        {"total":3}
    ]});
    let summary = parse_summary(&value);
    assert_eq!(summary.len(), 2);
    assert_eq!(summary[0].serial, "CAM1");
    assert_eq!(summary[0].total, 7);
    assert_eq!(summary[0].top_id.as_deref(), Some("m-7"));
    assert_eq!(summary[0].utc_offset, Some(3600));
    assert_eq!(summary[1].top_id, None);
    assert!(parse_summary(&json!({"meta":{"code":200}})).is_empty());
}

#[test]
fn list_fixture_maps_records_and_pictures() {
    let value = json!({"hasNext":true, "message":[
        {"msgId":"a1","deviceSerial":"CAM1","channel":1,"subType":2402,"time":1_700_000_005_000_i64,
         "title":" Person\n\tdetected \u{7}","pic":"https://pictures.example.invalid/a1_e",
         "picCrypt":2,"picChecksum":CHECKSUM,"ext":{"alarmType":"10120"}},
        {"msgId":"../../etc","deviceSerial":"CAM1","time":1_700_000_000_000_i64,"subType":2701,
         "ext":{"alarmType":0}},
        {"deviceSerial":"CAM1","channel":2,"timeStr":"2023-11-14 22:00:00","ext":{"alarmType":10002}},
        {"title":"no identity"}
    ]});
    let (messages, has_next) = parse_list(&value);
    assert!(has_next);
    assert_eq!(messages.len(), 3);
    let first = &messages[0];
    assert_eq!(first.record.id, "a1");
    assert_eq!(first.record.occurred_at, 1_700_000_005);
    assert_eq!(first.record.alarm_type, 10_120);
    assert_eq!(first.record.category, "person");
    assert_eq!(first.record.title, "Person detected");
    assert_eq!(first.channel, Some(1));
    let picture = first.picture.as_ref().unwrap_or_else(|| panic!("picture"));
    assert_eq!(
        (picture.crypt, picture.checksum.as_deref()),
        (2, Some(CHECKSUM))
    );
    assert!(!format!("{picture:?}").contains("example"));
    assert!(model::is_token(&messages[1].record.id));
    assert!(messages[1].record.id.starts_with('h'));
    assert_eq!(messages[1].record.category, "doorbell");
    assert!(messages[2].record.id.starts_with('h'));
    assert_eq!(messages[2].record.category, "motion");
    assert!(!parse_list(&json!({"hasNext":false,"message":[]})).1);
}

#[test]
fn category_mapping_follows_the_research_table() {
    for (alarm, sub, expected) in [
        (10_002, 0, "motion"),
        (10_140, 0, "motion"),
        (10_014, 0, "motion"),
        (10_010, 0, "person"),
        (10_131, 0, "person"),
        (10_130, 0, "vehicle"),
        (10_016, 0, "doorbell"),
        (0, 2_701, "doorbell"),
        (10_003, 0, "sound"),
        (15_003, 0, "pet"),
        (30_010, 0, "offline"),
        (10_071, 0, "offline"),
        (12_021, 0, "tamper"),
        (0, 2_404, "vehicle"),
        (99_999, 0, "other"),
    ] {
        assert_eq!(category(alarm, sub), expected, "{alarm}/{sub}");
    }
}

#[test]
fn titles_and_ids_are_bounded() {
    let long = "x".repeat(500);
    assert_eq!(
        model::sanitize_title(&long).chars().count(),
        model::MAX_TITLE_CHARS
    );
    assert!(!model::is_token(&"a".repeat(model::MAX_ID_CHARS + 1)));
    assert!(!model::is_token("a/b"));
    assert!(!model::is_token(""));
    let (messages, _) = parse_list(&json!({"message":[{"msgId":"x".repeat(200)}]}));
    assert!(model::is_token(&messages[0].record.id));
}

#[test]
fn checksum_crypt_decodes_headed_and_container_payloads() {
    let key = checksum_key();
    let headed = headed_block(&key, JPEG);
    assert_eq!(
        decode_picture(&headed, 2, Some(CHECKSUM), None),
        Ok(JPEG.to_vec())
    );

    let mut hikpic = b"hikpic".to_vec();
    hikpic.extend_from_slice(&1_u16.to_be_bytes());
    hikpic.push(0);
    hikpic.extend_from_slice(&u32::try_from(headed.len()).unwrap_or(0).to_be_bytes());
    hikpic.extend_from_slice(&headed);
    assert_eq!(
        decode_picture(&hikpic, 2, Some(CHECKSUM), None),
        Ok(JPEG.to_vec())
    );

    let mut other = JPEG.to_vec();
    other.insert(4, 0x42);
    let mut little = b"hiklittlepic".to_vec();
    little.extend_from_slice(&3_u16.to_be_bytes());
    for (index, clear) in [(0_u16, other.as_slice()), (1, JPEG)] {
        let block = headed_block(&key, clear);
        little.extend_from_slice(&0_u32.to_be_bytes());
        little.extend_from_slice(&index.to_be_bytes());
        little.extend_from_slice(&u32::try_from(block.len()).unwrap_or(0).to_be_bytes());
        little.extend_from_slice(&block);
        little.extend_from_slice(&1_u16.to_be_bytes());
        little.extend_from_slice(&4_u32.to_be_bytes());
        little.extend_from_slice(&[0, 0, 0, 0]);
    }
    assert_eq!(
        decode_picture(&little, 2, Some(CHECKSUM), None),
        Ok(JPEG.to_vec())
    );
}

#[test]
fn checksum_crypt_fails_closed() {
    let key = checksum_key();
    let headed = headed_block(&key, JPEG);
    let wrong = "ffeeddccbbaa99887766554433221100";
    assert!(decode_picture(&headed, 2, Some(wrong), None).is_err());
    assert_eq!(
        decode_picture(&headed, 2, None, None),
        Err(PictureError::Invalid)
    );
    assert_eq!(
        decode_picture(&headed, 2, Some("zz"), None),
        Err(PictureError::Invalid)
    );
    assert_eq!(
        decode_picture(b"unknown-format", 2, Some(CHECKSUM), None),
        Err(PictureError::Unsupported)
    );
    let mut odd = headed;
    odd.push(0);
    assert_eq!(
        decode_picture(&odd, 2, Some(CHECKSUM), None),
        Err(PictureError::Unsupported)
    );
    let mut truncated = b"hikpic\0\x01\0".to_vec();
    truncated.extend_from_slice(&999_u32.to_be_bytes());
    assert_eq!(
        decode_picture(&truncated, 2, Some(CHECKSUM), None),
        Err(PictureError::Invalid)
    );
    assert_eq!(
        decode_picture(JPEG, 9, None, None),
        Err(PictureError::Unsupported)
    );
    assert_eq!(
        decode_picture(b"<html>", 0, None, None),
        Err(PictureError::Unsupported)
    );
    assert_eq!(decode_picture(JPEG, 0, None, None), Ok(JPEG.to_vec()));
}

#[test]
fn device_key_crypt_reuses_the_snapshot_decryptor() {
    let code = "ABCDEF";
    let mut key = [0_u8; 16];
    key[..code.len()].copy_from_slice(code.as_bytes());
    let first = hex::encode(Md5::digest(code.as_bytes()));
    let mut payload = b"hikencodepicture".to_vec();
    payload.extend_from_slice(hex::encode(Md5::digest(first.as_bytes())).as_bytes());
    payload.extend(encrypt(&key, JPEG));
    assert_eq!(
        decode_picture(&payload, 1, None, Some(code)),
        Ok(JPEG.to_vec())
    );
    assert!(decode_picture(&payload, 1, None, Some("WRONG1")).is_err());
    assert_eq!(
        decode_picture(&payload, 1, None, None),
        Err(PictureError::Invalid)
    );
}
