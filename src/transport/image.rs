use aes::Aes128;
use cbc::Decryptor;
use cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};
use md5::{Digest as _, Md5};
use serde_json::Value;

use crate::error::BridgeError;

const HEADER: &[u8] = b"hikencodepicture";
const HASH_BYTES: usize = 32;

pub fn first_image_url(value: &Value) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in [
                "picUrl",
                "picURL",
                "imageUrl",
                "imageURL",
                "captureUrl",
                "captureURL",
                "pic",
                "pics",
                "image",
                "url",
            ] {
                if let Some(found) = map.get(key).and_then(http_part) {
                    return Some(found);
                }
            }
            map.values().find_map(first_image_url)
        }
        Value::Array(values) => values.iter().find_map(first_image_url),
        _ => None,
    }
}

fn http_part(value: &Value) -> Option<String> {
    value.as_str()?.split(';').find_map(|part| {
        let candidate = part.trim();
        candidate
            .starts_with("https://")
            .then(|| candidate.to_owned())
            .or_else(|| {
                candidate
                    .starts_with("http://")
                    .then(|| candidate.to_owned())
            })
    })
}

pub fn decrypt_image(input: &[u8], password: &str) -> Result<Vec<u8>, BridgeError> {
    let Some(start) = input
        .windows(HEADER.len())
        .position(|window| window == HEADER)
    else {
        return Ok(input.to_vec());
    };
    let mut output = Vec::new();
    let data = &input[start..];
    let mut cursor = 0;
    while cursor < data.len() {
        if data.get(cursor..cursor + HEADER.len()) != Some(HEADER) {
            break;
        }
        let search = cursor + HEADER.len();
        let end = data[search..]
            .windows(HEADER.len())
            .position(|window| window == HEADER)
            .map_or(data.len(), |position| search + position);
        output.extend(decrypt_block(&data[cursor..end], password)?);
        cursor = end;
    }
    if output.is_empty() {
        return Err(BridgeError::Upstream(
            "invalid encrypted image payload".into(),
        ));
    }
    Ok(output)
}

fn decrypt_block(block: &[u8], password: &str) -> Result<Vec<u8>, BridgeError> {
    let hash_end = HEADER.len() + HASH_BYTES;
    if block.len() <= hash_end {
        return Err(BridgeError::Upstream("encrypted image is truncated".into()));
    }
    let first = hex::encode(Md5::digest(password.as_bytes()));
    let expected = hex::encode(Md5::digest(first.as_bytes()));
    if block[HEADER.len()..hash_end] != *expected.as_bytes() {
        return Err(BridgeError::Upstream(
            "camera image key was rejected".into(),
        ));
    }
    let aligned = (block.len() - hash_end) / 16 * 16;
    let mut ciphertext = block[hash_end..hash_end + aligned].to_vec();
    let mut key = [0_u8; 16];
    let key_bytes = password.as_bytes();
    key[..key_bytes.len().min(16)].copy_from_slice(&key_bytes[..key_bytes.len().min(16)]);
    let iv = [
        b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', 0, 0, 0, 0, 0, 0, 0, 0,
    ];
    Decryptor::<Aes128>::new(&key.into(), &iv.into())
        .decrypt_padded_mut::<Pkcs7>(&mut ciphertext)
        .map(Vec::from)
        .map_err(|_| BridgeError::Upstream("encrypted image padding is invalid".into()))
}

pub fn jpeg_slice(data: &[u8]) -> Result<Vec<u8>, BridgeError> {
    let Some(start) = data
        .windows(3)
        .position(|value| value == [0xff, 0xd8, 0xff])
    else {
        return Err(BridgeError::Upstream(
            "snapshot payload is not a JPEG".into(),
        ));
    };
    let Some(end) = data.windows(2).rposition(|value| value == [0xff, 0xd9]) else {
        return Err(BridgeError::Upstream(
            "snapshot payload is not a JPEG".into(),
        ));
    };
    if end < start {
        return Err(BridgeError::Upstream(
            "snapshot payload is not a JPEG".into(),
        ));
    }
    Ok(data[start..end + 2].to_vec())
}
