use crate::error::BridgeError;

#[path = "video_cipher.rs"]
mod cipher;
#[path = "video_rtp.rs"]
mod rtp;
use rtp::RtpNalDecryptor;

const MAX_PENDING_BYTES: usize = 4 * 1024 * 1024;
const MAX_CODEC_PROBE_PACKETS: usize = 400;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VideoCodec {
    H264,
    Hevc,
}

/// One parsed RTP packet; the payload borrows from the VTM body.
pub(super) struct RtpPacket<'a> {
    pub(super) payload_type: u8,
    pub(super) marker: bool,
    pub(super) timestamp: u32,
    pub(super) payload: &'a [u8],
}

/// What one RTP packet yielded once the video payload type is known.
pub(super) enum RtpOutput {
    /// Annex-B bytes of the video access unit stamped `timestamp`.
    Video {
        timestamp: u32,
        marker: bool,
        data: Vec<u8>,
    },
    /// A packet of any other payload type, for the audio track to classify.
    Other {
        payload_type: u8,
        timestamp: u32,
        payload: Vec<u8>,
    },
}

/// Depacketizes RTP video, decrypting it when a verification code is given.
/// The video payload type is the first one whose payload names a codec.
pub(super) struct RtpVideo {
    key: Option<[u8; 16]>,
    codec: Option<VideoCodec>,
    payload_type: Option<u8>,
    buffered: Vec<Vec<u8>>,
    buffered_bytes: usize,
    decryptor: Option<RtpNalDecryptor>,
}

impl RtpVideo {
    pub(super) fn encrypted(verification_code: &str) -> Result<Self, BridgeError> {
        if verification_code.is_empty() || verification_code.len() > 64 {
            return Err(BridgeError::Configuration(
                "camera verification code length is invalid".into(),
            ));
        }
        let mut key = [0_u8; 16];
        let value = verification_code.as_bytes();
        key[..value.len().min(16)].copy_from_slice(&value[..value.len().min(16)]);
        Ok(Self::with_key(Some(key)))
    }

    /// A camera that sends RTP without encryption: no key, no classification.
    pub(super) const fn clear() -> Self {
        Self::with_key(None)
    }

    const fn with_key(key: Option<[u8; 16]>) -> Self {
        Self {
            key,
            codec: None,
            payload_type: None,
            buffered: Vec::new(),
            buffered_bytes: 0,
            decryptor: None,
        }
    }

    pub(super) const fn codec(&self) -> Option<VideoCodec> {
        self.codec
    }

    pub(super) fn feed(&mut self, packet: &[u8]) -> Result<Vec<RtpOutput>, BridgeError> {
        reject_non_rtp_container(packet)?;
        let rtp = parse_rtp(packet)?;
        if let Some(selected) = self.payload_type {
            return self.route(&rtp, selected);
        }

        self.buffered_bytes = self.buffered_bytes.saturating_add(packet.len());
        if self.buffered_bytes > MAX_PENDING_BYTES || self.buffered.len() >= MAX_CODEC_PROBE_PACKETS
        {
            return Err(BridgeError::Upstream(
                "RTP video codec could not be identified within its bound".into(),
            ));
        }
        self.buffered.push(packet.to_vec());
        let Some(codec) = detect_codec(rtp.payload.first().copied()) else {
            return Ok(Vec::new());
        };
        self.codec = Some(codec);
        self.payload_type = Some(rtp.payload_type);
        self.decryptor = Some(RtpNalDecryptor::new(codec, self.key));
        let pending = std::mem::take(&mut self.buffered);
        self.buffered_bytes = 0;
        let mut output = Vec::new();
        for candidate in pending {
            let candidate = parse_rtp(&candidate)?;
            // Packets of other types seen before the codec are only dropped.
            if candidate.payload_type == rtp.payload_type {
                output.extend(self.route(&candidate, rtp.payload_type)?);
            }
        }
        Ok(output)
    }

    fn route(&mut self, rtp: &RtpPacket<'_>, video: u8) -> Result<Vec<RtpOutput>, BridgeError> {
        if rtp.payload_type != video {
            return Ok(vec![RtpOutput::Other {
                payload_type: rtp.payload_type,
                timestamp: rtp.timestamp,
                payload: rtp.payload.to_vec(),
            }]);
        }
        let chunks = self
            .decryptor
            .as_mut()
            .ok_or_else(|| BridgeError::Upstream("video depacketizer is unavailable".into()))?
            .feed(rtp.timestamp, rtp.payload)?;
        // Held NALs released now keep their own timestamps; only this
        // packet's access unit can be closed by its marker bit.
        let mut output: Vec<RtpOutput> = chunks
            .into_iter()
            .map(|(timestamp, data)| RtpOutput::Video {
                timestamp,
                marker: false,
                data,
            })
            .collect();
        match output.last_mut() {
            Some(RtpOutput::Video {
                timestamp, marker, ..
            }) if *timestamp == rtp.timestamp => *marker = rtp.marker,
            _ => output.push(RtpOutput::Video {
                timestamp: rtp.timestamp,
                marker: rtp.marker,
                data: Vec::new(),
            }),
        }
        Ok(output)
    }
}

/// True when a VTM payload is an RTP version 2 packet. MPEG-PS (`00`), the
/// `IMKH` header (`0x49`) and MPEG-TS (`0x47`) can never parse as one, and the
/// RTP path itself identifies the codec, buffering as long as it needs.
pub(super) fn is_rtp(packet: &[u8]) -> bool {
    parse_rtp(packet).is_ok()
}

fn reject_non_rtp_container(packet: &[u8]) -> Result<(), BridgeError> {
    let format = if packet.starts_with(b"\x00\x00\x01\xba") {
        Some("MPEG-PS")
    } else if packet.first() == Some(&0x47) {
        Some("MPEG-TS")
    } else {
        None
    };
    if let Some(format) = format {
        return Err(BridgeError::Upstream(format!(
            "{format} video is not supported on the RTP path of this camera profile"
        )));
    }
    Ok(())
}

fn parse_rtp(packet: &[u8]) -> Result<RtpPacket<'_>, BridgeError> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return Err(BridgeError::Upstream(
            "media is not a valid RTP packet".into(),
        ));
    }
    let payload_type = packet[1] & 0x7f;
    let mut offset = 12 + usize::from(packet[0] & 0x0f) * 4;
    if offset > packet.len() {
        return Err(BridgeError::Upstream("RTP header is truncated".into()));
    }
    if packet[0] & 0x10 != 0 {
        if offset + 4 > packet.len() {
            return Err(BridgeError::Upstream(
                "RTP extension header is truncated".into(),
            ));
        }
        let words = usize::from(u16::from_be_bytes([packet[offset + 2], packet[offset + 3]]));
        offset = offset
            .saturating_add(4)
            .saturating_add(words.saturating_mul(4));
        if offset > packet.len() {
            return Err(BridgeError::Upstream("RTP extension is truncated".into()));
        }
    }
    let padding = if packet[0] & 0x20 == 0 {
        0
    } else {
        usize::from(
            *packet
                .last()
                .ok_or_else(|| BridgeError::Upstream("RTP padding is missing".into()))?,
        )
    };
    let end = packet
        .len()
        .checked_sub(padding)
        .filter(|end| *end >= offset)
        .ok_or_else(|| BridgeError::Upstream("RTP padding is invalid".into()))?;
    Ok(RtpPacket {
        payload_type,
        marker: packet[1] & 0x80 != 0,
        timestamp: u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]),
        payload: &packet[offset..end],
    })
}

fn detect_codec(first: Option<u8>) -> Option<VideoCodec> {
    let first = first?;
    let h264_type = first & 0x1f;
    if matches!(h264_type, 24 | 28) || matches!(first, 0x65 | 0x67 | 0x68) {
        return Some(VideoCodec::H264);
    }
    let hevc_type = (first >> 1) & 0x3f;
    if matches!(hevc_type, 48 | 49) || matches!(first, 0x40 | 0x42 | 0x44) {
        return Some(VideoCodec::Hevc);
    }
    None
}

#[cfg(test)]
#[path = "video_tests.rs"]
mod tests;
