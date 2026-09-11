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

impl VideoCodec {
    pub(super) const fn ffmpeg_name(self) -> &'static str {
        match self {
            Self::H264 => "h264",
            Self::Hevc => "hevc",
        }
    }
}

pub(super) struct EncryptedRtpVideo {
    key: [u8; 16],
    codec: Option<VideoCodec>,
    payload_type: Option<u8>,
    buffered: Vec<Vec<u8>>,
    buffered_bytes: usize,
    decryptor: Option<RtpNalDecryptor>,
}

impl EncryptedRtpVideo {
    pub(super) fn new(verification_code: &str) -> Result<Self, BridgeError> {
        if verification_code.is_empty() || verification_code.len() > 64 {
            return Err(BridgeError::Configuration(
                "camera verification code length is invalid".into(),
            ));
        }
        let mut key = [0_u8; 16];
        let value = verification_code.as_bytes();
        key[..value.len().min(16)].copy_from_slice(&value[..value.len().min(16)]);
        Ok(Self {
            key,
            codec: None,
            payload_type: None,
            buffered: Vec::new(),
            buffered_bytes: 0,
            decryptor: None,
        })
    }

    pub(super) const fn codec(&self) -> Option<VideoCodec> {
        self.codec
    }

    pub(super) fn feed(&mut self, packet: &[u8]) -> Result<Vec<u8>, BridgeError> {
        reject_non_rtp_container(packet)?;
        let (payload_type, payload) = rtp_payload(packet)?;
        if let Some(selected) = self.payload_type {
            if selected != payload_type {
                return Ok(Vec::new());
            }
            return self
                .decryptor
                .as_mut()
                .ok_or_else(|| BridgeError::Upstream("video decryptor is unavailable".into()))?
                .feed(payload);
        }

        self.buffered_bytes = self.buffered_bytes.saturating_add(packet.len());
        if self.buffered_bytes > MAX_PENDING_BYTES || self.buffered.len() >= MAX_CODEC_PROBE_PACKETS
        {
            return Err(BridgeError::Upstream(
                "encrypted RTP codec could not be identified within its bound".into(),
            ));
        }
        self.buffered.push(packet.to_vec());
        let Some(codec) = detect_codec(payload.first().copied()) else {
            return Ok(Vec::new());
        };
        self.codec = Some(codec);
        self.payload_type = Some(payload_type);
        self.decryptor = Some(RtpNalDecryptor::new(codec, self.key));
        let pending = std::mem::take(&mut self.buffered);
        self.buffered_bytes = 0;
        let mut output = Vec::new();
        for candidate in pending {
            let (candidate_type, candidate_payload) = rtp_payload(&candidate)?;
            if candidate_type == payload_type {
                output.extend(
                    self.decryptor
                        .as_mut()
                        .ok_or_else(|| {
                            BridgeError::Upstream("video decryptor is unavailable".into())
                        })?
                        .feed(candidate_payload)?,
                );
            }
        }
        Ok(output)
    }
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
            "encrypted {format} video is not supported by this camera profile"
        )));
    }
    Ok(())
}

fn rtp_payload(packet: &[u8]) -> Result<(u8, &[u8]), BridgeError> {
    if packet.len() < 12 || packet[0] >> 6 != 2 {
        return Err(BridgeError::Upstream(
            "encrypted media is not a valid RTP packet".into(),
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
    Ok((payload_type, &packet[offset..end]))
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
