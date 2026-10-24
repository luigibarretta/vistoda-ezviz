//! Holding video until the audio question is settled, and timestamps that
//! survive the encrypted decryptor's start-up queue.

use aes::{
    Aes128,
    cipher::{BlockEncrypt, KeyInit, generic_array::GenericArray},
};

use super::tests::{AUDIO_PT, VIDEO_PT, aac, feed_all, hevc, pes_pts, rtp, video_frames};
use super::*;
use crate::transport::ts_mux::{AUDIO_PID, VIDEO_PID};

const CODE: &str = "VERIFYCODE1";

fn encrypt(value: &[u8]) -> Vec<u8> {
    let mut key = [0_u8; 16];
    key[..CODE.len()].copy_from_slice(CODE.as_bytes());
    let cipher = Aes128::new(GenericArray::from_slice(&key));
    let mut output = value.to_vec();
    let blocks = output.len() / 16 * 16;
    for block in output[..blocks].chunks_exact_mut(16) {
        cipher.encrypt_block(GenericArray::from_mut_slice(block));
    }
    output
}

/// The clear two-byte HEVC header followed by an encrypted body.
fn encrypted_hevc(nal_type: u8, body: &[u8]) -> Vec<u8> {
    [&[nal_type << 1, 1][..], &encrypt(body)].concat()
}

#[test]
fn the_first_pmt_already_names_the_audio_stream() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let mut packets = video_frames(0, 3);
    for index in 0..8_u32 {
        packets.push(rtp(AUDIO_PT, true, index * 1_024, &aac(0x01)));
    }
    let mut output = Vec::new();
    for packet in &packets[..3] {
        output.extend(
            stream
                .feed(packet)
                .unwrap_or_else(|error| panic!("{error}")),
        );
    }
    assert!(output.is_empty(), "video waits while audio is undecided");
    for packet in &packets[3..] {
        output.extend(
            stream
                .feed(packet)
                .unwrap_or_else(|error| panic!("{error}")),
        );
    }
    let pmt = &output[188..376];
    assert_eq!(u16::from(pmt[1] & 0x1f) << 8 | u16::from(pmt[2]), 0x1000);
    assert!(
        pmt.windows(3).any(|w| w == [0x0f, 0xe1, 0x01]),
        "AAC in first PMT"
    );
    // VPS+IDR, P1 and P2 were held; the eighth AAC packet releases them
    // together with all eight confirming frames.
    assert_eq!(pes_pts(&output, VIDEO_PID), [90_000, 96_000, 102_000]);
    let expected: Vec<u64> = (0..8).map(|frame| 102_000 + frame * 5_760).collect();
    assert_eq!(pes_pts(&output, AUDIO_PID), expected);
}

#[test]
fn video_without_audio_is_released_after_the_wait() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let mut output = Vec::new();
    for packet in video_frames(0, 17) {
        output.extend(
            stream
                .feed(&packet)
                .unwrap_or_else(|error| panic!("{error}")),
        );
    }
    // The sixteenth frame reaches the one-second wait and releases all held
    // video; the seventeenth is written at once. No finish() is needed.
    assert_eq!(pes_pts(&output, VIDEO_PID).len(), 17);
    assert!(stream.audio.gave_up);
    assert!(pes_pts(&output, AUDIO_PID).is_empty());
}

#[test]
fn encrypted_start_up_keeps_one_timestamp_per_access_unit() {
    let mut stream = RtpToTs::new(RtpVideo::encrypted(CODE).unwrap_or_else(|e| panic!("{e}")));
    stream.audio.give_up();
    let mut packets = vec![rtp(
        VIDEO_PT,
        false,
        0,
        &encrypted_hevc(32, &[&[0x0c][..], &[1; 31][..]].concat()),
    )];
    packets.push(rtp(
        VIDEO_PT,
        true,
        0,
        &encrypted_hevc(19, &[&[0x26, 0][..], &[2; 30][..]].concat()),
    ));
    for frame in 1..13_u32 {
        let index = u8::try_from(frame).unwrap_or_default();
        let body = [&[2, 1, index][..], &[3; 29][..]].concat();
        packets.push(rtp(
            VIDEO_PT,
            true,
            frame * 6_000,
            &encrypted_hevc(1, &body),
        ));
    }
    let output = feed_all(&mut stream, &packets);
    let expected: Vec<u64> = (0..13).map(|frame| 90_000 + frame * 6_000).collect();
    assert_eq!(pes_pts(&output, VIDEO_PID), expected);
    // The keyframe is written first and decrypted: its slice body is clear.
    let start = output
        .windows(6)
        .position(|w| w == [0, 0, 0, 1, 19 << 1, 1])
        .unwrap_or_else(|| panic!("IDR missing"));
    assert_eq!(&output[start + 6..start + 8], [0x26, 0]);
}

/// One AAC-hbr payload carrying `first_bytes.len()` four-byte Access Units.
fn aac_frames(first_bytes: &[u8]) -> Vec<u8> {
    let count = u16::try_from(first_bytes.len()).unwrap_or_default();
    let mut payload = (count * 16).to_be_bytes().to_vec();
    for index in 0..count {
        payload.extend_from_slice(&(4_u16 << 3 | (index & 0x07)).to_be_bytes());
    }
    for first in first_bytes {
        payload.extend_from_slice(&[*first, 0x40, 0x20, 0x00]);
    }
    payload
}

#[test]
fn packets_with_several_frames_confirm_audio_by_frame_count() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let mut packets = video_frames(0, 2);
    // FFmpeg's packetizer sends about five frames per packet: two packets
    // already hold the eight frames that confirm the track.
    packets.push(rtp(AUDIO_PT, true, 0, &aac_frames(&[0x01; 5])));
    packets.push(rtp(AUDIO_PT, true, 5 * 1_024, &aac_frames(&[0x01; 5])));
    let output = feed_all(&mut stream, &packets);
    assert_eq!(stream.audio.payload_type, Some(AUDIO_PT));
    // Both packets' ten frames are written, 1024 samples apart. The first
    // packet arrived at video PTS 96000 holding five frames, so its first
    // frame began four frames earlier.
    let expected: Vec<u64> = (0..10).map(|frame| 72_960 + frame * 5_760).collect();
    assert_eq!(pes_pts(&output, AUDIO_PID), expected);
}

#[test]
fn a_packet_with_one_foreign_frame_does_not_count() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let mut packets = video_frames(0, 2);
    for index in 0..4_u32 {
        packets.push(rtp(AUDIO_PT, true, index, &aac_frames(&[0x01, 0xe0, 0x01])));
    }
    let _output = feed_all(&mut stream, &packets);
    assert_eq!(stream.audio.payload_type, None);
}

#[test]
fn confirming_audio_is_interleaved_with_held_video_by_pts() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    // Audio and video arrive together, as from a camera: each video frame
    // (6000 ticks) is followed by one AAC frame (5760 ticks).
    let mut packets = Vec::new();
    for (frame, packet) in video_frames(0, 9).into_iter().enumerate() {
        packets.push(packet);
        let index = u32::try_from(frame).unwrap_or_default();
        packets.push(rtp(AUDIO_PT, true, index * 1_024, &aac(0x01)));
    }
    let output = feed_all(&mut stream, &packets);
    // Within the first second of the file both streams already appear.
    let pids: Vec<u16> = output
        .chunks(188)
        .map(|packet| u16::from(packet[1] & 0x1f) << 8 | u16::from(packet[2]))
        .collect();
    let first_audio = pids.iter().position(|pid| *pid == AUDIO_PID);
    let third_video = pids
        .iter()
        .enumerate()
        .filter(|(_, pid)| **pid == VIDEO_PID)
        .nth(2)
        .map(|(index, _)| index);
    assert!(first_audio.is_some_and(|audio| third_video.is_some_and(|video| audio < video)));
}

#[test]
fn frame_ticks_match_the_sample_rate() {
    use crate::transport::aac_rtp::{FRAME_SAMPLES, SAMPLE_RATE};
    assert_eq!(
        i64::from(FRAME_SAMPLES) * 90_000 / i64::from(SAMPLE_RATE),
        5_760
    );
}

#[test]
fn audio_confirmed_before_the_first_keyframe_is_anchored_at_video_start() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    // The codec is known (VPS) but the keyframe completes a second later,
    // while sixteen AAC frames (about one second) arrive.
    let mut packets = vec![rtp(VIDEO_PT, false, 0, &hevc(32, &[0x0c]))];
    for index in 0..16_u32 {
        packets.push(rtp(AUDIO_PT, true, index * 1_024, &aac(0x01)));
    }
    packets.push(rtp(VIDEO_PT, true, 0, &hevc(19, &[0xaf])));
    packets.push(rtp(AUDIO_PT, true, 16 * 1_024, &aac(0x01)));
    let output = feed_all(&mut stream, &packets);
    assert_eq!(pes_pts(&output, VIDEO_PID), [90_000]);
    // The first frame written after the keyframe starts on its timeline,
    // not a second later.
    assert_eq!(pes_pts(&output, AUDIO_PID), [90_000]);
}
