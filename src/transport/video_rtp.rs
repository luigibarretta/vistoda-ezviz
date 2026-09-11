use crate::error::BridgeError;

use super::{VideoCodec, cipher::NalDecryptor};

const MAX_FRAGMENT_BYTES: usize = 2 * 1024 * 1024;

pub(super) struct RtpNalDecryptor {
    codec: VideoCodec,
    nal: NalDecryptor,
    fragment: Option<Vec<u8>>,
}

impl RtpNalDecryptor {
    pub(super) const fn new(codec: VideoCodec, key: [u8; 16]) -> Self {
        Self {
            codec,
            nal: NalDecryptor::new(codec, key),
            fragment: None,
        }
    }

    pub(super) fn feed(&mut self, payload: &[u8]) -> Result<Vec<u8>, BridgeError> {
        match self.codec {
            VideoCodec::H264 => self.feed_h264(payload),
            VideoCodec::Hevc => self.feed_hevc(payload),
        }
    }

    fn feed_h264(&mut self, payload: &[u8]) -> Result<Vec<u8>, BridgeError> {
        let Some(&first) = payload.first() else {
            return Ok(Vec::new());
        };
        match first & 0x1f {
            28 => {
                if payload.len() < 2 {
                    return Err(BridgeError::Upstream("H.264 FU-A is truncated".into()));
                }
                let fu = payload[1];
                let start = fu & 0x80 != 0;
                let end = fu & 0x40 != 0;
                if start {
                    self.fragment = Some(vec![(first & 0xe0) | (fu & 0x1f)]);
                }
                if let Some(fragment) = &mut self.fragment {
                    fragment.extend_from_slice(&payload[2..]);
                    bound_fragment(fragment)?;
                }
                if end {
                    return self.finish_fragment();
                }
                Ok(Vec::new())
            }
            24 => self.feed_aggregation(&payload[1..]),
            _ => self.nal.feed(payload),
        }
    }

    fn feed_hevc(&mut self, payload: &[u8]) -> Result<Vec<u8>, BridgeError> {
        if payload.len() < 2 {
            return Ok(Vec::new());
        }
        match (payload[0] >> 1) & 0x3f {
            49 => {
                if payload.len() < 3 {
                    return Err(BridgeError::Upstream("HEVC FU is truncated".into()));
                }
                let fu = payload[2];
                let start = fu & 0x80 != 0;
                let end = fu & 0x40 != 0;
                if start {
                    self.fragment =
                        Some(vec![(payload[0] & 0x81) | ((fu & 0x3f) << 1), payload[1]]);
                }
                if let Some(fragment) = &mut self.fragment {
                    fragment.extend_from_slice(&payload[3..]);
                    bound_fragment(fragment)?;
                }
                if end {
                    return self.finish_fragment();
                }
                Ok(Vec::new())
            }
            48 => self.feed_aggregation(&payload[2..]),
            _ => self.nal.feed(payload),
        }
    }

    fn feed_aggregation(&mut self, mut payload: &[u8]) -> Result<Vec<u8>, BridgeError> {
        let mut output = Vec::new();
        while !payload.is_empty() {
            if payload.len() < 2 {
                return Err(BridgeError::Upstream(
                    "RTP aggregation length is truncated".into(),
                ));
            }
            let size = usize::from(u16::from_be_bytes([payload[0], payload[1]]));
            payload = &payload[2..];
            if size == 0 || size > payload.len() {
                return Err(BridgeError::Upstream(
                    "RTP aggregation unit is invalid".into(),
                ));
            }
            output.extend(self.nal.feed(&payload[..size])?);
            payload = &payload[size..];
        }
        Ok(output)
    }

    fn finish_fragment(&mut self) -> Result<Vec<u8>, BridgeError> {
        let fragment = self
            .fragment
            .take()
            .ok_or_else(|| BridgeError::Upstream("RTP fragment ended before it started".into()))?;
        self.nal.feed(&fragment)
    }
}

fn bound_fragment(fragment: &[u8]) -> Result<(), BridgeError> {
    if fragment.len() > MAX_FRAGMENT_BYTES {
        return Err(BridgeError::Capacity(
            "encrypted RTP fragment exceeded its byte bound".into(),
        ));
    }
    Ok(())
}
