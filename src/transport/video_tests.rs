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
        let mut video = RtpVideo::encrypted(CODE).unwrap_or_else(|error| panic!("{error}"));
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
            output.extend(video_bytes(
                video
                    .feed(&rtp(
                        nal,
                        u16::try_from(index).unwrap_or_else(|error| panic!("{error}")),
                    ))
                    .unwrap_or_else(|error| panic!("{error}")),
            ));
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
    let mut video = RtpVideo::encrypted("WRONG").unwrap_or_else(|error| panic!("{error}"));
    let vps = encrypted_hevc(32, &[&[0x0c][..], &[1; 31][..]].concat());
    assert!(matches!(
        video.feed(&rtp(&vps, 1)),
        Err(BridgeError::KeyRejected)
    ));
}

#[test]
fn rtp_parser_honours_csrc_extension_and_padding() {
    let mut packet = vec![0xb1, 96, 0, 1];
    packet.extend_from_slice(&[0; 8]);
    packet.extend_from_slice(&[0; 4]);
    packet.extend_from_slice(&[0xbe, 0xde, 0, 1, 1, 2, 3, 4]);
    packet.extend_from_slice(&[0x40, 1, 0, 0, 0, 4]);
    let parsed = parse_rtp(&packet).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(parsed.payload, &[0x40, 1]);
    assert_eq!(parsed.payload_type, 96);
    assert!(!parsed.marker);
}

fn video_bytes(outputs: Vec<RtpOutput>) -> Vec<u8> {
    outputs
        .into_iter()
        .flat_map(|output| match output {
            RtpOutput::Video { data, .. } => data,
            RtpOutput::Other { .. } => Vec::new(),
        })
        .collect()
}

fn stamped(payload_type: u8, marker: bool, timestamp: u32, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x80, payload_type | u8::from(marker) << 7, 0, 1];
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&[0; 4]);
    packet.extend_from_slice(payload);
    packet
}

#[test]
fn clear_rtp_passes_through_with_its_timestamp_and_marker() {
    let mut video = RtpVideo::clear();
    let vps = [&hevc_header(32)[..], &[0x0c, 1, 2, 3]].concat();
    let outputs = video
        .feed(&stamped(96, true, 3_000, &vps))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(video.codec(), Some(VideoCodec::Hevc));
    let [
        RtpOutput::Video {
            timestamp,
            marker,
            data,
        },
    ] = outputs.as_slice()
    else {
        panic!("one video output expected");
    };
    assert_eq!((*timestamp, *marker), (3_000, true));
    assert_eq!(data, &[START_CODE, &vps].concat());
}

#[test]
fn clear_h264_fu_a_is_reassembled() {
    let mut video = RtpVideo::clear();
    let start = [0x7c, 0x85, 1, 2];
    let end = [0x7c, 0x45, 3, 4];
    let mut output = video_bytes(
        video
            .feed(&stamped(96, false, 9, &start))
            .unwrap_or_else(|error| panic!("{error}")),
    );
    output.extend(video_bytes(
        video
            .feed(&stamped(96, true, 9, &end))
            .unwrap_or_else(|error| panic!("{error}")),
    ));
    assert_eq!(video.codec(), Some(VideoCodec::H264));
    assert_eq!(output, [START_CODE, &[0x65, 1, 2, 3, 4]].concat());
}

#[test]
fn other_payload_types_are_routed_once_the_video_type_is_known() {
    let mut video = RtpVideo::clear();
    let audio = [0x00, 0x10, 0x00, 0x10, 0x01, 0x40];
    // Audio before the video codec is known is dropped, not misread as video.
    assert!(
        video
            .feed(&stamped(104, false, 1, &audio))
            .unwrap_or_else(|error| panic!("{error}"))
            .is_empty()
    );
    let sps = [0x67, 0x64, 0x00, 0x1f];
    let _video = video.feed(&stamped(96, true, 2, &sps));
    let outputs = video
        .feed(&stamped(104, false, 160, &audio))
        .unwrap_or_else(|error| panic!("{error}"));
    let [
        RtpOutput::Other {
            payload_type,
            timestamp,
            payload,
        },
    ] = outputs.as_slice()
    else {
        panic!("one routed packet expected");
    };
    assert_eq!((*payload_type, *timestamp), (104, 160));
    assert_eq!(payload, &audio);
}

#[test]
fn rtp_is_told_apart_from_program_and_transport_streams() {
    assert!(is_rtp(&stamped(96, false, 0, &[0x40, 1, 0x0c])));
    assert!(is_rtp(&stamped(
        104,
        false,
        0,
        &[0x00, 0x10, 0x00, 0x10, 0x01, 0x40]
    )));
    assert!(!is_rtp(
        b"\x00\x00\x01\xba\x44\x00\x04\x00\x04\x01\x01\x89\xc3\xf8"
    ));
    assert!(!is_rtp(&[0x47; 188]));
    assert!(!is_rtp(b"IMKH\x01\x01\x00\x00\x04\x00\x00\x01"));
    // A first packet that is SEI, AUD or an unknown type is still RTP: the
    // RTP path finds the codec later instead of passing RTP off as MPEG-PS.
    assert!(is_rtp(&stamped(96, false, 0, &[0x06, 0x05, 0xff])));
    assert!(is_rtp(&stamped(96, false, 0, &[0x09, 0xf0])));
    // A truncated header is not RTP.
    assert!(!is_rtp(&[0x80, 96, 0, 1, 0, 0]));
}
