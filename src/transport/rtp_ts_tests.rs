use super::*;
use crate::transport::ts_mux::{AUDIO_PID, VIDEO_PID};

pub(super) const VIDEO_PT: u8 = 96;
pub(super) const AUDIO_PT: u8 = 104;

pub(super) fn rtp(payload_type: u8, marker: bool, timestamp: u32, payload: &[u8]) -> Vec<u8> {
    let mut packet = vec![0x80, payload_type | u8::from(marker) << 7, 0, 0];
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&[0; 4]);
    packet.extend_from_slice(payload);
    packet
}

pub(super) fn hevc(nal_type: u8, body: &[u8]) -> Vec<u8> {
    [&[nal_type << 1, 1][..], body].concat()
}

/// One AAC-hbr payload carrying a single mono Access Unit.
pub(super) fn aac(first_byte: u8) -> Vec<u8> {
    let unit = [first_byte, 0x40, 0x20, 0x00];
    let size = u16::try_from(unit.len()).unwrap_or_default() << 3;
    [&[0x00, 0x10][..], &size.to_be_bytes(), &unit].concat()
}

/// PTS of every PES that starts on `pid`, in stream order.
pub(super) fn pes_pts(stream: &[u8], pid: u16) -> Vec<u64> {
    assert_eq!(stream.len() % 188, 0);
    stream
        .chunks(188)
        .filter(|packet| u16::from(packet[1] & 0x1f) << 8 | u16::from(packet[2]) == pid)
        .filter(|packet| packet[1] & 0x40 != 0)
        .map(|packet| {
            let start = if packet[3] & 0x20 == 0 {
                4
            } else {
                5 + usize::from(packet[4])
            };
            let pts = &packet[start + 9..start + 14];
            (u64::from(pts[0] >> 1) & 0x07) << 30
                | u64::from(pts[1]) << 22
                | u64::from(pts[2] >> 1) << 15
                | u64::from(pts[3]) << 7
                | u64::from(pts[4] >> 1)
        })
        .collect()
}

pub(super) fn feed_all(stream: &mut RtpToTs, packets: &[Vec<u8>]) -> Vec<u8> {
    let mut output = Vec::new();
    for packet in packets {
        output.extend(
            stream
                .feed(packet)
                .unwrap_or_else(|error| panic!("{error}")),
        );
    }
    output.extend(stream.finish());
    output
}

/// A 15 fps HEVC stream starting at `base`, keyframe first, one NAL per frame
/// except the keyframe, which carries VPS and IDR in two packets.
pub(super) fn video_frames(base: u32, frames: u32) -> Vec<Vec<u8>> {
    let mut packets = vec![rtp(VIDEO_PT, false, base, &hevc(32, &[0x0c, 1]))];
    packets.push(rtp(VIDEO_PT, true, base, &hevc(19, &[0xaf, 2])));
    for frame in 1..frames {
        let timestamp = base.wrapping_add(frame * 6_000);
        packets.push(rtp(VIDEO_PT, true, timestamp, &hevc(1, &[0xd0, 3])));
    }
    packets
}

#[test]
fn video_pts_follows_the_rtp_clock_not_an_assumed_frame_rate() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let output = feed_all(&mut stream, &video_frames(1_000, 5));
    assert!(stream.started());
    let pts = pes_pts(&output, VIDEO_PID);
    assert_eq!(pts, [90_000, 96_000, 102_000, 108_000, 114_000]);
}

#[test]
fn the_rtp_timestamp_wraps_without_a_jump() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let output = feed_all(&mut stream, &video_frames(u32::MAX - 6_000, 4));
    assert_eq!(
        pes_pts(&output, VIDEO_PID),
        [90_000, 96_000, 102_000, 108_000]
    );
}

#[test]
fn units_before_the_first_keyframe_are_dropped() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let mut packets = vec![
        rtp(VIDEO_PT, true, 10, &hevc(32, &[0x0c, 9])),
        rtp(VIDEO_PT, true, 20, &hevc(1, &[0xd0, 9])),
    ];
    packets.extend(video_frames(500, 2));
    let output = feed_all(&mut stream, &packets);
    assert_eq!(pes_pts(&output, VIDEO_PID), [90_000, 96_000]);
}

#[test]
fn an_access_unit_without_a_marker_closes_on_the_next_timestamp() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    // No audio question pending, so video is written as soon as it closes.
    stream.audio.give_up();
    let packets = vec![
        rtp(VIDEO_PT, false, 0, &hevc(32, &[0x0c])),
        rtp(VIDEO_PT, false, 0, &hevc(19, &[0xaf])),
        rtp(VIDEO_PT, false, 6_000, &hevc(1, &[0xd0])),
    ];
    let mut output = Vec::new();
    for packet in &packets {
        output.extend(
            stream
                .feed(packet)
                .unwrap_or_else(|error| panic!("{error}")),
        );
    }
    assert_eq!(
        pes_pts(&output, VIDEO_PID),
        [90_000],
        "second unit still open"
    );
    output.extend(stream.finish());
    assert_eq!(pes_pts(&output, VIDEO_PID), [90_000, 96_000]);
}

#[test]
fn aac_is_confirmed_then_stamped_on_its_own_clock_from_the_video_anchor() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let mut packets = video_frames(0, 3);
    for index in 0..12_u32 {
        packets.push(rtp(AUDIO_PT, true, 70_000 + index * 1_024, &aac(0x01)));
    }
    let output = feed_all(&mut stream, &packets);
    let audio = pes_pts(&output, AUDIO_PID);
    // The eighth packet confirms the track; the seven before it are written
    // too, from the video PTS current when the first of them arrived.
    assert_eq!(audio.len(), 12);
    assert_eq!(audio[0], 102_000, "anchored to the latest video PTS");
    for pair in audio.windows(2) {
        assert_eq!(pair[1] - pair[0], 5_760, "1024 samples at 16 kHz");
    }
    assert_eq!(stream.audio_frames(), (12, 0));
    let frame_start = output
        .windows(3)
        .position(|window| window == [0xff, 0xf1, 0x60]);
    assert!(
        frame_start.is_some(),
        "an AAC-LC 16 kHz ADTS header is muxed"
    );
}

#[test]
fn a_payload_type_that_does_not_read_as_aac_never_becomes_audio() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let mut packets = video_frames(0, 2);
    // Encrypted or foreign data: the first element is never SCE or CPE.
    for index in 0..500_u32 {
        packets.push(rtp(AUDIO_PT, true, index * 1_024, &aac(0xe0)));
    }
    let output = feed_all(&mut stream, &packets);
    assert!(pes_pts(&output, AUDIO_PID).is_empty());
    assert!(stream.audio.gave_up);
}

#[test]
fn audio_before_video_starts_is_ignored() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let mut packets = Vec::new();
    for index in 0..10_u32 {
        packets.push(rtp(AUDIO_PT, true, index * 1_024, &aac(0x01)));
    }
    packets.extend(video_frames(0, 2));
    let output = feed_all(&mut stream, &packets);
    assert!(pes_pts(&output, AUDIO_PID).is_empty());
    assert_eq!(stream.audio.probed, 0);
}

#[test]
fn an_access_unit_that_never_closes_is_bounded() {
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let slice = hevc(1, &vec![0xd0; 60_000]);
    let mut result = Ok(Vec::new());
    for _ in 0..200 {
        result = stream.feed(&rtp(VIDEO_PT, false, 7, &hevc(32, &[0x0c])));
        if result.is_err() {
            break;
        }
        result = stream.feed(&rtp(VIDEO_PT, false, 7, &slice));
        if result.is_err() {
            break;
        }
    }
    assert!(matches!(result, Err(BridgeError::Capacity(_))));
}
