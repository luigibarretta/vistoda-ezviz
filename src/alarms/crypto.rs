//! Alarm picture decoding for EZVIZ `picCrypt` 0, 1 and 2.
//!
//! `picCrypt=2` follows the official app's checksum-key path: the key is the
//! `picChecksum` hex string decoded to 16 bytes, AES-128-CBC/PKCS#7 with IV
//! `"01234567"` plus eight zero bytes. Payloads are either a 48-byte header
//! followed by ciphertext, or the big-endian `hikpic` / `hiklittlepic`
//! containers whose blocks each carry that same 48-byte header. No owned
//! sample has been captured yet, so every unknown shape fails closed.

use aes::Aes128;
use cbc::Decryptor;
use cipher::{BlockDecryptMut, KeyIvInit, block_padding::Pkcs7};

use crate::transport::{decrypt_image, jpeg_slice};

const BLOCK_HEADER: usize = 48;
const IV: [u8; 16] = [
    b'0', b'1', b'2', b'3', b'4', b'5', b'6', b'7', 0, 0, 0, 0, 0, 0, 0, 0,
];
const MAX_CONTAINER_BLOCKS: usize = 16;

#[derive(Debug, PartialEq, Eq)]
pub enum PictureError {
    /// The payload or `picCrypt` mode is not a recognized format.
    Unsupported,
    /// A recognized format failed to decrypt or validate.
    Invalid,
}

/// Decodes a downloaded alarm picture into a validated JPEG.
pub fn decode_picture(
    raw: &[u8],
    crypt: i64,
    checksum: Option<&str>,
    device_key: Option<&str>,
) -> Result<Vec<u8>, PictureError> {
    match crypt {
        0 => validated_jpeg(raw),
        1 => {
            let key = device_key.ok_or(PictureError::Invalid)?;
            let clear = decrypt_image(raw, key).map_err(|_| PictureError::Invalid)?;
            validated_jpeg(&clear)
        }
        2 => decrypt_checksum_picture(raw, checksum.ok_or(PictureError::Invalid)?),
        _ => Err(PictureError::Unsupported),
    }
}

/// Requires the JPEG SOI marker at offset zero and trims to the last EOI.
pub fn validated_jpeg(data: &[u8]) -> Result<Vec<u8>, PictureError> {
    if !data.starts_with(&[0xff, 0xd8, 0xff]) {
        return Err(PictureError::Unsupported);
    }
    jpeg_slice(data).map_err(|_| PictureError::Invalid)
}

/// Decodes a `picCrypt=2` payload with the hex key carried in `picChecksum`.
pub fn decrypt_checksum_picture(raw: &[u8], checksum: &str) -> Result<Vec<u8>, PictureError> {
    let key = checksum_key(checksum).ok_or(PictureError::Invalid)?;
    if raw.starts_with(&[0xff, 0xd8, 0xff]) {
        return validated_jpeg(raw);
    }
    if raw.starts_with(b"hiklittlepic") {
        return little_container(raw, &key);
    }
    if raw.starts_with(b"hikpic") {
        // Layout: magic(6) version(u16) reserved(1) length(u32) block.
        let length = read_u32(raw, 9).ok_or(PictureError::Invalid)?;
        let block = slice(raw, 13, length).ok_or(PictureError::Invalid)?;
        return decrypt_block(block, &key);
    }
    decrypt_block(raw, &key)
}

/// Walks a `hiklittlepic` container and prefers the picture with index 1.
fn little_container(raw: &[u8], key: &[u8; 16]) -> Result<Vec<u8>, PictureError> {
    let version = read_u16(raw, 12).ok_or(PictureError::Invalid)?;
    let mut cursor = 14;
    let mut first = None;
    for _ in 0..MAX_CONTAINER_BLOCKS {
        if cursor >= raw.len() {
            break;
        }
        let index = read_u16(raw, cursor + 4).ok_or(PictureError::Invalid)?;
        let length = read_u32(raw, cursor + 6).ok_or(PictureError::Invalid)?;
        let block = slice(raw, cursor + 10, length).ok_or(PictureError::Invalid)?;
        cursor += 10 + block.len();
        if version == 3 {
            let detection = read_u32(raw, cursor + 2).ok_or(PictureError::Invalid)?;
            cursor += 6 + slice(raw, cursor + 6, detection)
                .ok_or(PictureError::Invalid)?
                .len();
        }
        if let Ok(picture) = decrypt_block(block, key) {
            if index == 1 {
                return Ok(picture);
            }
            first.get_or_insert(picture);
        }
    }
    first.ok_or(PictureError::Invalid)
}

fn decrypt_block(block: &[u8], key: &[u8; 16]) -> Result<Vec<u8>, PictureError> {
    let ciphertext = block.get(BLOCK_HEADER..).ok_or(PictureError::Unsupported)?;
    if ciphertext.is_empty() || ciphertext.len() % 16 != 0 {
        return Err(PictureError::Unsupported);
    }
    let mut buffer = ciphertext.to_vec();
    let clear = Decryptor::<Aes128>::new(key.into(), &IV.into())
        .decrypt_padded_mut::<Pkcs7>(&mut buffer)
        .map_err(|_| PictureError::Invalid)?;
    validated_jpeg(clear)
}

/// Decodes up to 16 key bytes from hex, zero-padding short keys like the app.
fn checksum_key(checksum: &str) -> Option<[u8; 16]> {
    let text = checksum.trim();
    if text.len() < 2 || text.len() > 128 || !text.len().is_multiple_of(2) {
        return None;
    }
    let decoded = hex::decode(text).ok()?;
    let mut key = [0_u8; 16];
    let used = decoded.len().min(16);
    key[..used].copy_from_slice(&decoded[..used]);
    Some(key)
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    let bytes = data.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_be_bytes([bytes[0], bytes[1]]))
}

fn read_u32(data: &[u8], offset: usize) -> Option<usize> {
    let bytes = data.get(offset..offset.checked_add(4)?)?;
    usize::try_from(u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])).ok()
}

fn slice(data: &[u8], offset: usize, length: usize) -> Option<&[u8]> {
    data.get(offset..offset.checked_add(length)?)
}
