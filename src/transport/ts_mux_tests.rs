use super::*;

fn packets(stream: &[u8]) -> Vec<&[u8]> {
    assert_eq!(stream.len() % PACKET, 0, "stream is not packet aligned");
    stream.chunks(PACKET).collect()
}

fn pid(packet: &[u8]) -> u16 {
    u16::from(packet[1] & 0x1f) << 8 | u16::from(packet[2])
}

fn payload(packet: &[u8]) -> &[u8] {
    if packet[3] & 0x20 == 0 {
        &packet[4..]
    } else {
        &packet[5 + usize::from(packet[4])..]
    }
}

/// Reassembles the PES packets of one PID, split on `payload_unit_start`.
fn pes_units(stream: &[u8], wanted: u16) -> Vec<Vec<u8>> {
    let mut units: Vec<Vec<u8>> = Vec::new();
    for packet in packets(stream).into_iter().filter(|p| pid(p) == wanted) {
        if packet[1] & 0x40 != 0 {
            units.push(Vec::new());
        }
        units
            .last_mut()
            .unwrap_or_else(|| panic!("payload before a unit start"))
            .extend_from_slice(payload(packet));
    }
    units
}

fn decode_pts(bytes: &[u8]) -> u64 {
    (u64::from(bytes[0] >> 1) & 0x07) << 30
        | u64::from(bytes[1]) << 22
        | u64::from(bytes[2] >> 1) << 15
        | u64::from(bytes[3]) << 7
        | u64::from(bytes[4] >> 1)
}

#[test]
fn pat_matches_the_well_known_ffmpeg_section() {
    let pat = section(0x00, 1, 0, &[0x00, 0x01, 0xf0, 0x00]);
    assert_eq!(
        pat,
        [
            0x00, 0xb0, 0x0d, 0x00, 0x01, 0xc1, 0x00, 0x00, 0x00, 0x01, 0xf0, 0x00, 0x2a, 0xb1,
            0x04, 0xb2
        ]
    );
}

#[test]
fn pts_round_trips_across_the_33_bit_range() {
    for pts in [0, 1, 90_000, 0x1_2345_6789, PTS_MASK] {
        let bytes = pts_bytes(pts);
        assert_eq!(bytes[0] & 0xf1, 0x21);
        assert_eq!(decode_pts(&bytes), pts);
    }
}

#[test]
fn every_payload_size_is_packetized_and_reassembled_exactly() {
    for size in 0..(PAYLOAD * 3) {
        let mut muxer = TsMuxer::new(VideoCodec::Hevc);
        let body: Vec<u8> = (0..size)
            .map(|value| u8::try_from(value % 251).unwrap_or_default())
            .collect();
        let stream = muxer.video(90_000, size % 2 == 0, &body);
        let units = pes_units(&stream, VIDEO_PID);
        assert_eq!(units.len(), 1, "size {size}");
        let unit = &units[0];
        assert_eq!(&unit[..4], b"\x00\x00\x01\xe0");
        assert_eq!(decode_pts(&unit[9..14]), 90_000);
        assert_eq!(&unit[14..21], b"\x00\x00\x00\x01\x46\x01\x50");
        assert_eq!(&unit[21..], body.as_slice(), "size {size}");
    }
}

#[test]
fn keyframes_resend_tables_and_mark_random_access_with_pcr() {
    let mut muxer = TsMuxer::new(VideoCodec::H264);
    let first = muxer.video(180_000, true, &[0x65; 400]);
    let delta = muxer.video(183_000, false, &[0x41; 10]);
    let key = muxer.video(186_000, true, &[0x65; 10]);
    let pids = |stream: &[u8]| packets(stream).iter().map(|p| pid(p)).collect::<Vec<_>>();
    assert_eq!(&pids(&first)[..2], [0x0000, PMT_PID]);
    assert!(!pids(&delta).contains(&0x0000));
    assert_eq!(&pids(&key)[..2], [0x0000, PMT_PID]);
    let video = packets(&first)[2];
    assert_eq!(video[1] & 0x40, 0x40);
    assert_eq!(video[3] & 0x30, 0x30);
    assert_eq!(video[5] & 0x50, 0x50, "random access and PCR flags");
    let pcr = u64::from(video[6]) << 25
        | u64::from(video[7]) << 17
        | u64::from(video[8]) << 9
        | u64::from(video[9]) << 1
        | u64::from(video[10] >> 7);
    assert_eq!(pcr, 180_000 - PCR_LEAD);
    let unit = &pes_units(&first, VIDEO_PID)[0];
    assert_eq!(&unit[14..20], b"\x00\x00\x00\x01\x09\xf0", "H.264 AUD");
}

#[test]
fn continuity_counters_advance_per_pid_and_wrap() {
    let mut muxer = TsMuxer::new(VideoCodec::Hevc);
    let mut stream = Vec::new();
    for index in 0..40_u64 {
        stream.extend(muxer.video(index * 6_000, false, &[0x02; 300]));
    }
    let counters: Vec<u8> = packets(&stream)
        .into_iter()
        .filter(|p| pid(p) == VIDEO_PID)
        .map(|p| p[3] & 0x0f)
        .collect();
    for pair in counters.windows(2) {
        assert_eq!(pair[1], (pair[0] + 1) & 0x0f);
    }
    assert!(counters.len() > 16, "the counter must have wrapped");
}

#[test]
fn audio_is_announced_with_a_new_pmt_version_and_bounded_pes() {
    let mut muxer = TsMuxer::new(VideoCodec::Hevc);
    let _start = muxer.video(90_000, true, &[0x26; 20]);
    muxer.enable_audio();
    let frame = [0xff, 0xf1, 0x60, 0x40, 0x02, 0x1f, 0xfc, 0x21];
    let stream = muxer.audio(91_000, &frame);
    let tables = packets(&stream);
    assert_eq!(pid(tables[1]), PMT_PID);
    let pmt = &tables[1][5..];
    assert_eq!((pmt[5] >> 1) & 0x1f, 1, "PMT version");
    let section_length = usize::from(pmt[1] & 0x0f) << 8 | usize::from(pmt[2]);
    let section = &pmt[..3 + section_length];
    let crc = crc32_mpeg2(&section[..section.len() - 4]);
    assert_eq!(&section[section.len() - 4..], crc.to_be_bytes());
    assert!(
        section.windows(3).any(|w| w == [0x0f, 0xe1, 0x01]),
        "AAC entry"
    );
    let unit = &pes_units(&stream, AUDIO_PID)[0];
    assert_eq!(&unit[..4], b"\x00\x00\x01\xc0");
    let length = usize::from(u16::from_be_bytes([unit[4], unit[5]]));
    assert_eq!(length, unit.len() - 6);
    assert_eq!(&unit[14..], frame);
}

#[test]
fn an_access_unit_delimiter_from_the_camera_is_not_doubled() {
    let mut muxer = TsMuxer::new(VideoCodec::H264);
    let unit = [0, 0, 0, 1, 0x09, 0xf0, 0, 0, 0, 1, 0x65, 0x88];
    let stream = muxer.video(90_000, true, &unit);
    let pes = &pes_units(&stream, VIDEO_PID)[0];
    assert_eq!(&pes[14..], unit);
}
