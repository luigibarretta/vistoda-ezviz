//! Minimal MPEG-TS muxer for depacketized RTP media.
//!
//! RTP carries the camera's own 90 kHz video clock and its audio sample
//! clock. Writing the transport stream here keeps those timestamps; a raw
//! Annex-B pipe into `FFmpeg` would replace them with an assumed 25 fps.
//! Scope is deliberately small: one program, one video stream (H.264 or
//! HEVC, no B-frames, so DTS equals PTS) and an optional ADTS AAC stream.

use super::video::VideoCodec;

#[path = "ts_mux_crc.rs"]
mod crc;
pub(super) use crc::crc32_mpeg2;

const PACKET: usize = 188;
const PAYLOAD: usize = PACKET - 4;
const PMT_PID: u16 = 0x1000;
pub(super) const VIDEO_PID: u16 = 0x0100;
pub(super) const AUDIO_PID: u16 = 0x0101;
const PTS_MASK: u64 = (1 << 33) - 1;
/// PCR runs this far behind the video PTS, the decoder's buffering margin.
const PCR_LEAD: u64 = 45_000;

pub(super) struct TsMuxer {
    codec: VideoCodec,
    audio: bool,
    pmt_version: u8,
    tables_due: bool,
    continuity: [u8; 4],
}

impl TsMuxer {
    pub(super) const fn new(codec: VideoCodec) -> Self {
        Self {
            codec,
            audio: false,
            pmt_version: 0,
            tables_due: true,
            continuity: [0; 4],
        }
    }

    /// Declares the audio stream; the next packet carries a new PMT version.
    pub(super) const fn enable_audio(&mut self) {
        if !self.audio {
            self.audio = true;
            self.pmt_version = (self.pmt_version + 1) & 0x1f;
            self.tables_due = true;
        }
    }

    /// One video access unit in Annex-B form, stamped with a 90 kHz PTS.
    pub(super) fn video(&mut self, pts: u64, keyframe: bool, access_unit: &[u8]) -> Vec<u8> {
        let mut output = self.tables(keyframe);
        let delimiter: &[u8] = match self.codec {
            VideoCodec::H264 => b"\x00\x00\x00\x01\x09\xf0",
            VideoCodec::Hevc => b"\x00\x00\x00\x01\x46\x01\x50",
        };
        // Cameras that already send an access unit delimiter keep their own.
        let mut body = if access_unit.get(4) == delimiter.get(4) {
            Vec::with_capacity(access_unit.len())
        } else {
            delimiter.to_vec()
        };
        body.extend_from_slice(access_unit);
        let packet = pes(0xe0, pts, &body, false);
        let pcr = pts.wrapping_sub(PCR_LEAD) & PTS_MASK;
        self.packetize(VIDEO_PID, &packet, Some(pcr), keyframe, &mut output);
        output
    }

    /// One or more ADTS frames that share a 90 kHz PTS.
    pub(super) fn audio(&mut self, pts: u64, adts: &[u8]) -> Vec<u8> {
        let mut output = self.tables(false);
        let packet = pes(0xc0, pts, adts, true);
        self.packetize(AUDIO_PID, &packet, None, false, &mut output);
        output
    }

    fn tables(&mut self, keyframe: bool) -> Vec<u8> {
        if !self.tables_due && !keyframe {
            return Vec::new();
        }
        self.tables_due = false;
        let program_association = section(
            0x00,
            1,
            0,
            &[0x00, 0x01, 0xe0 | high(PMT_PID), low(PMT_PID)],
        );
        let stream_type = match self.codec {
            VideoCodec::H264 => 0x1b,
            VideoCodec::Hevc => 0x24,
        };
        let mut body = vec![0xe0 | high(VIDEO_PID), low(VIDEO_PID), 0xf0, 0x00];
        let mut streams = vec![(stream_type, VIDEO_PID)];
        if self.audio {
            streams.push((0x0f, AUDIO_PID));
        }
        for (kind, pid) in streams {
            body.extend_from_slice(&[kind, 0xe0 | high(pid), low(pid), 0xf0, 0x00]);
        }
        let pmt = section(0x02, 1, self.pmt_version, &body);
        let mut output = self.psi(0x0000, &program_association);
        output.extend(self.psi(PMT_PID, &pmt));
        output
    }

    fn psi(&mut self, pid: u16, section: &[u8]) -> Vec<u8> {
        let mut packet = vec![0x47, 0x40 | high(pid), low(pid), 0x10 | self.next_cc(pid)];
        packet.push(0x00);
        packet.extend_from_slice(section);
        packet.resize(PACKET, 0xff);
        packet
    }

    fn packetize(
        &mut self,
        pid: u16,
        pes: &[u8],
        pcr: Option<u64>,
        random_access: bool,
        output: &mut Vec<u8>,
    ) {
        let mut offset = 0;
        while offset < pes.len() {
            let first = offset == 0;
            let mut adaptation = if first && (pcr.is_some() || random_access) {
                let mut field = vec![u8::from(random_access) << 6 | u8::from(pcr.is_some()) << 4];
                if let Some(pcr) = pcr {
                    field.extend_from_slice(&pcr_bytes(pcr));
                }
                Some(field)
            } else {
                None
            };
            let used = adaptation.as_ref().map_or(0, |field| field.len() + 1);
            let remaining = pes.len() - offset;
            if remaining < PAYLOAD - used {
                let stuffing = PAYLOAD - used - remaining;
                match &mut adaptation {
                    Some(field) => field.resize(field.len() + stuffing, 0xff),
                    None if stuffing == 1 => adaptation = Some(Vec::new()),
                    None => {
                        let mut field = vec![0x00];
                        field.resize(stuffing - 1, 0xff);
                        adaptation = Some(field);
                    }
                }
            }
            let control = if adaptation.is_some() { 0x30 } else { 0x10 };
            let start = if first { 0x40 } else { 0x00 };
            output.extend_from_slice(&[0x47, start | high(pid), low(pid)]);
            output.push(control | self.next_cc(pid));
            if let Some(field) = adaptation {
                output.push(u8::try_from(field.len()).unwrap_or(u8::MAX));
                output.extend_from_slice(&field);
            }
            let room = PACKET - (output.len() % PACKET);
            let take = room.min(remaining);
            output.extend_from_slice(&pes[offset..offset + take]);
            offset += take;
        }
    }

    const fn next_cc(&mut self, pid: u16) -> u8 {
        let slot = match pid {
            0x0000 => 0,
            PMT_PID => 1,
            VIDEO_PID => 2,
            _ => 3,
        };
        let value = self.continuity[slot];
        self.continuity[slot] = (value + 1) & 0x0f;
        value
    }
}

fn pes(stream_id: u8, pts: u64, payload: &[u8], bounded: bool) -> Vec<u8> {
    let length = if bounded {
        u16::try_from(payload.len() + 8).unwrap_or(0)
    } else {
        0
    };
    let mut output = vec![0x00, 0x00, 0x01, stream_id];
    output.extend_from_slice(&length.to_be_bytes());
    output.extend_from_slice(&[0x80, 0x80, 0x05]);
    output.extend_from_slice(&pts_bytes(pts));
    output.extend_from_slice(payload);
    output
}

fn section(table_id: u8, id: u16, version: u8, body: &[u8]) -> Vec<u8> {
    // Five header bytes after the length, the body and a four-byte CRC.
    let length = u16::try_from(5 + body.len() + 4).unwrap_or(0x3fd);
    let mut output = vec![table_id, 0xb0 | high(length), low(length)];
    output.extend_from_slice(&id.to_be_bytes());
    output.extend_from_slice(&[0xc1 | (version & 0x1f) << 1, 0x00, 0x00]);
    output.extend_from_slice(body);
    let crc = crc32_mpeg2(&output);
    output.extend_from_slice(&crc.to_be_bytes());
    output
}

pub(super) const fn pts_bytes(pts: u64) -> [u8; 5] {
    let pts = pts & PTS_MASK;
    [
        0x21 | byte(pts >> 30) << 1 & 0x0e,
        byte(pts >> 22),
        byte(pts >> 14) & 0xfe | 1,
        byte(pts >> 7),
        byte(pts << 1) & 0xfe | 1,
    ]
}

const fn pcr_bytes(base: u64) -> [u8; 6] {
    let base = base & PTS_MASK;
    [
        byte(base >> 25),
        byte(base >> 17),
        byte(base >> 9),
        byte(base >> 1),
        byte(base & 1) << 7 | 0x7e,
        0x00,
    ]
}

/// Least significant byte, without a truncating cast.
const fn byte(value: u64) -> u8 {
    value.to_be_bytes()[7]
}

const fn high(value: u16) -> u8 {
    value.to_be_bytes()[0] & 0x1f
}

const fn low(value: u16) -> u8 {
    value.to_be_bytes()[1]
}

#[cfg(test)]
#[path = "ts_mux_tests.rs"]
mod tests;
