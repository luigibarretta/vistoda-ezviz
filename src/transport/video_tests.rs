use aes::{
    Aes128,
    cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray},
};

use super::*;

const CODE: &str = "VERIFYCODE1";
const START_CODE: &[u8] = b"\x00\x00\x00\x01";

fn key() -> [u8; 16] {
    let mut key = [0_u8; 16];
    key[..CODE.len()].copy_from_slice(CODE.as_bytes());
    key
}

fn encrypt(value: &[u8]) -> Vec<u8> {
    let mut output = value.to_vec();
    let blocks = output.len() / 16 * 16;
    let cipher = Aes128::new(GenericArray::from_slice(&key()));
    for block in output[..blocks].chunks_exact_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    output
}

fn hevc_header(nal_type: u8) -> [u8; 2] {
    [nal_type << 1, 1]
}

fn rtp(payload: &[u8], sequence: u16) -> Vec<u8> {
    let [high, low] = sequence.to_be_bytes();
    let mut packet = vec![0x80, 96, high, low];
    packet.extend_from_slice(&[0; 8]);
    packet.extend_from_slice(payload);
    packet
}

fn encrypted_hevc(nal_type: u8, body: &[u8]) -> Vec<u8> {
    let mut nal = hevc_header(nal_type).to_vec();
    nal.extend(encrypt(body));
    nal
}

#[test]
fn encrypted_and_clear_inter_frames_are_classified_without_reordering() {
    for encrypted_inter in [false, true] {
        let mut video = EncryptedRtpVideo::new(CODE).unwrap_or_else(|error| panic!("{error}"));
        let mut packets = vec![encrypted_hevc(32, &[&[0x0c][..], &[1; 31][..]].concat())];
        packets.push(encrypted_hevc(19, &[&[0x26, 0][..], &[2; 30][..]].concat()));
        for index in 0..12_u8 {
            let body = [&[2, 1, index][..], &[3; 29][..]].concat();
            let mut nal = hevc_header(1).to_vec();
            nal.extend(if encrypted_inter {
                encrypt(&body)
            } else {
                body
            });
            packets.push(nal);
        }
        let mut output = Vec::new();
        for (index, nal) in packets.iter().enumerate() {
            output.extend(
                video
                    .feed(&rtp(
                        nal,
                        u16::try_from(index).unwrap_or_else(|error| panic!("{error}")),
                    ))
                    .unwrap_or_else(|error| panic!("{error}")),
            );
        }
        assert_eq!(video.codec(), Some(VideoCodec::Hevc));
        let vps = [START_CODE, &hevc_header(32), &[0x0c]].concat();
        let inter = [START_CODE, &hevc_header(1), &[2, 1, 3]].concat();
        let vps_position = output.windows(vps.len()).position(|window| window == vps);
        let inter_position = output
            .windows(inter.len())
            .position(|window| window == inter);
        assert!(
            vps_position
                .is_some_and(|position| inter_position.is_some_and(|later| position < later))
        );
    }
}

#[test]
fn wrong_key_fails_closed_without_emitting_ciphertext() {
    let mut video = EncryptedRtpVideo::new("WRONG").unwrap_or_else(|error| panic!("{error}"));
    let vps = encrypted_hevc(32, &[&[0x0c][..], &[1; 31][..]].concat());
    assert!(matches!(
        video.feed(&rtp(&vps, 1)),
        Err(BridgeError::Authentication)
    ));
}

#[test]
fn rtp_parser_honours_csrc_extension_and_padding() {
    let mut packet = vec![0xb1, 96, 0, 1];
    packet.extend_from_slice(&[0; 8]);
    packet.extend_from_slice(&[0; 4]);
    packet.extend_from_slice(&[0xbe, 0xde, 0, 1, 1, 2, 3, 4]);
    packet.extend_from_slice(&[0x40, 1, 0, 0, 0, 4]);
    let (_, payload) = rtp_payload(&packet).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(payload, &[0x40, 1]);
}
