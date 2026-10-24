//! RFC 3640 `mpeg4-generic` AAC in `AAC-hbr` mode, as EZVIZ cameras send it.
//!
//! The camera's SDP would say `mode=AAC-hbr; sizelength=13; indexlength=3`
//! and `config=1408` (AAC-LC, 16 kHz, mono): each RTP payload opens with a
//! 16-bit AU-headers length, then one 16-bit header per Access Unit, then the
//! bare Access Units. They carry no ADTS header, so one is rebuilt here.
//! The sample rate is not in the payload and follows that family convention;
//! the channel count is read from each unit's first syntactic element.

/// Sample rate of the EZVIZ/Hikvision AAC profile when no SDP is available.
pub(super) const SAMPLE_RATE: u32 = 16_000;
/// MPEG-4 sampling frequency index of [`SAMPLE_RATE`].
const SAMPLE_RATE_INDEX: u8 = 8;
/// Samples per AAC-LC frame; one Access Unit is one frame.
pub(super) const FRAME_SAMPLES: u32 = 1024;
const MAX_FRAME_BYTES: usize = (1 << 13) - 1 - ADTS_HEADER;
const ADTS_HEADER: usize = 7;

/// The Access Units of one payload, or `None` when the payload is not a
/// complete, self-consistent `AAC-hbr` packet. Being strict is what lets the
/// session tell an audio payload type from any other one.
pub(super) fn access_units(payload: &[u8]) -> Option<Vec<&[u8]>> {
    let header_bits = usize::from(u16::from_be_bytes([*payload.first()?, *payload.get(1)?]));
    if header_bits == 0 || header_bits % 16 != 0 {
        return None;
    }
    let header_end = 2 + header_bits / 8;
    let headers = payload.get(2..header_end)?;
    let mut data = payload.get(header_end..)?;
    let mut units = Vec::with_capacity(headers.len() / 2);
    for header in headers.chunks_exact(2) {
        let size = usize::from(u16::from_be_bytes([header[0], header[1]]) >> 3);
        if size == 0 || size > data.len() || size > MAX_FRAME_BYTES {
            return None;
        }
        let (unit, rest) = data.split_at(size);
        units.push(unit);
        data = rest;
    }
    data.is_empty().then_some(units)
}

/// Channel count named by an Access Unit's first syntactic element:
/// a single channel element is mono, a channel pair element is stereo.
/// Anything else is not something this profile produces.
pub(super) fn channels(unit: &[u8]) -> Option<u8> {
    match unit.first()? >> 5 {
        0 => Some(1),
        1 => Some(2),
        _ => None,
    }
}

/// One ADTS frame: a seven-byte header without CRC, then the Access Unit.
pub(super) fn adts_frame(unit: &[u8], channels: u8) -> Vec<u8> {
    let length = u16::try_from(unit.len() + ADTS_HEADER).unwrap_or(u16::MAX) & 0x1fff;
    let [length_high, length_low] = length.to_be_bytes();
    let channels = channels & 0x07;
    let mut frame = vec![
        0xff,
        0xf1,
        // Profile 1 is AAC-LC (object type 2 minus one).
        0x40 | SAMPLE_RATE_INDEX << 2 | channels >> 2,
        (channels & 0x03) << 6 | length_high >> 3,
        length_high << 5 | length_low >> 3,
        (length_low & 0x07) << 5 | 0x1f,
        0xfc,
    ];
    frame.extend_from_slice(unit);
    frame
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(units: &[&[u8]]) -> Vec<u8> {
        let bits = u16::try_from(units.len() * 16).unwrap_or_default();
        let mut output = bits.to_be_bytes().to_vec();
        for (index, unit) in units.iter().enumerate() {
            let size = u16::try_from(unit.len()).unwrap_or_default();
            let index = u16::try_from(index).unwrap_or_default() & 0x07;
            output.extend_from_slice(&(size << 3 | index).to_be_bytes());
        }
        for unit in units {
            output.extend_from_slice(unit);
        }
        output
    }

    #[test]
    fn reads_one_and_several_access_units() {
        let first = [0x01, 0x40, 0x20];
        let second = [0x21, 0x10];
        assert_eq!(access_units(&payload(&[&first])), Some(vec![&first[..]]));
        assert_eq!(
            access_units(&payload(&[&first, &second])),
            Some(vec![&first[..], &second[..]])
        );
    }

    #[test]
    fn the_reporters_payload_prefix_is_one_unit_of_228_bytes() {
        // `00 10 07 20`: 16 header bits, one AU of 0x0720 >> 3 = 228 bytes.
        let mut packet = vec![0x00, 0x10, 0x07, 0x20];
        packet.extend_from_slice(&[0x01; 228]);
        let units = access_units(&packet).unwrap_or_default();
        assert_eq!(units.len(), 1);
        assert_eq!(units[0].len(), 228);
    }

    #[test]
    fn rejects_payloads_that_are_not_consistent_aac_hbr() {
        let unit = [0x01, 0x40];
        let mut long = payload(&[&unit]);
        long.push(0);
        let mut short = payload(&[&unit]);
        short.pop();
        for packet in [
            vec![],
            vec![0x00],
            vec![0x00, 0x00, 0x00],
            vec![0x00, 0x0c, 0x00, 0x10, 0x01],
            vec![0x00, 0x10, 0x00, 0x00],
            long,
            short,
            // A video NAL header is not a valid AU-headers length here.
            vec![0x40, 0x01, 0x0c, 0x01, 0xff],
        ] {
            assert_eq!(access_units(&packet), None, "{packet:02x?}");
        }
    }

    #[test]
    fn channel_count_comes_from_the_first_element() {
        assert_eq!(channels(&[0x01]), Some(1));
        assert_eq!(channels(&[0x21]), Some(2));
        assert_eq!(channels(&[0xe0]), None);
        assert_eq!(channels(&[]), None);
    }

    #[test]
    fn adts_header_matches_the_aac_lc_16k_layout() {
        let unit = [0x01; 228];
        let frame = adts_frame(&unit, 1);
        // FFmpeg's ADTS for AAC-LC 16 kHz mono, frame length 235.
        assert_eq!(&frame[..7], [0xff, 0xf1, 0x60, 0x40, 0x1d, 0x7f, 0xfc]);
        assert_eq!(&frame[7..], unit);
        let stereo = adts_frame(&[0x21], 2);
        assert_eq!(stereo[2] & 0x01, 0);
        assert_eq!(stereo[3] >> 6, 2);
        let length = usize::from(stereo[3] & 0x03) << 11
            | usize::from(stereo[4]) << 3
            | usize::from(stereo[5] >> 5);
        assert_eq!(length, 8);
    }
}
