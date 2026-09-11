use crate::error::BridgeError;

const PACK_START: &[u8] = b"\x00\x00\x01\xba";
const TS_PACKET_BYTES: usize = 188;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaFormat {
    ProgramStream,
    TransportStream,
}

impl MediaFormat {
    pub(super) fn from_media_type(value: &str) -> Result<Self, BridgeError> {
        match value {
            "video/mpeg" => Ok(Self::ProgramStream),
            "video/mp2t" => Ok(Self::TransportStream),
            _ => Err(BridgeError::Upstream(
                "recording media type is unsupported".into(),
            )),
        }
    }

    pub(super) const fn extension(self) -> &'static str {
        match self {
            Self::ProgramStream => "mpegps",
            Self::TransportStream => "ts",
        }
    }

    pub(super) fn matches_prefix(self, value: &[u8]) -> bool {
        match self {
            Self::ProgramStream => value.starts_with(PACK_START),
            Self::TransportStream => {
                value.len() > TS_PACKET_BYTES * 2
                    && value[0] == 0x47
                    && value[TS_PACKET_BYTES] == 0x47
                    && value[TS_PACKET_BYTES * 2] == 0x47
            }
        }
    }

    pub(super) fn detect(value: &[u8]) -> Option<Self> {
        [Self::ProgramStream, Self::TransportStream]
            .into_iter()
            .find(|format| format.matches_prefix(value))
    }
}

pub const PROBE_BYTES: usize = TS_PACKET_BYTES * 3;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_program_and_transport_streams_only() {
        assert_eq!(
            MediaFormat::detect(b"\x00\x00\x01\xba-media"),
            Some(MediaFormat::ProgramStream)
        );
        let mut transport = vec![0_u8; PROBE_BYTES];
        for offset in [0, TS_PACKET_BYTES, TS_PACKET_BYTES * 2] {
            transport[offset] = 0x47;
        }
        assert_eq!(
            MediaFormat::detect(&transport),
            Some(MediaFormat::TransportStream)
        );
        assert_eq!(MediaFormat::detect(b"not media"), None);
    }
}
