use crate::{
    error::BridgeError,
    vtm::{CHANNEL_ENCRYPTED_MESSAGE, CHANNEL_ENCRYPTED_STREAM, CHANNEL_MESSAGE, CHANNEL_STREAM},
};

const MAGIC: u8 = 0x24;
pub const HEADER_SIZE: usize = 8;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VtmPacket {
    pub channel: u8,
    pub sequence: u16,
    pub message_code: u16,
    pub body: Vec<u8>,
}

impl VtmPacket {
    #[must_use]
    pub const fn encrypted(&self) -> bool {
        matches!(
            self.channel,
            CHANNEL_ENCRYPTED_MESSAGE | CHANNEL_ENCRYPTED_STREAM
        )
    }
}

pub fn encode_packet(
    body: &[u8],
    channel: u8,
    sequence: u16,
    message_code: u16,
) -> Result<Vec<u8>, BridgeError> {
    let length = u16::try_from(body.len())
        .map_err(|_| BridgeError::Upstream("VTM packet body is too large".into()))?;
    let mut packet = Vec::with_capacity(HEADER_SIZE + body.len());
    packet.extend_from_slice(&[MAGIC, channel]);
    packet.extend_from_slice(&length.to_be_bytes());
    packet.extend_from_slice(&sequence.to_be_bytes());
    packet.extend_from_slice(&message_code.to_be_bytes());
    packet.extend_from_slice(body);
    Ok(packet)
}

pub fn decode_header(header: &[u8]) -> Result<(u8, usize, u16, u16), BridgeError> {
    if header.len() != HEADER_SIZE || header[0] != MAGIC {
        return Err(BridgeError::Upstream("invalid VTM packet header".into()));
    }
    let channel = header[1];
    if !matches!(
        channel,
        CHANNEL_MESSAGE | CHANNEL_STREAM | CHANNEL_ENCRYPTED_MESSAGE | CHANNEL_ENCRYPTED_STREAM
    ) {
        return Err(BridgeError::Upstream("unknown VTM packet channel".into()));
    }
    Ok((
        channel,
        usize::from(u16::from_be_bytes([header[2], header[3]])),
        u16::from_be_bytes([header[4], header[5]]),
        u16::from_be_bytes([header[6], header[7]]),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_roundtrip_matches_wire_header() {
        let packet = encode_packet(b"abc", CHANNEL_STREAM, 2, 0x13c)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(&packet[..8], &[0x24, 1, 0, 3, 0, 2, 1, 0x3c]);
        assert_eq!(
            decode_header(&packet[..8]).unwrap_or_else(|error| panic!("{error}")),
            (1, 3, 2, 0x13c)
        );
    }

    #[test]
    fn malformed_headers_are_rejected() {
        assert!(decode_header(&[0; HEADER_SIZE]).is_err());
        assert!(decode_header(&[MAGIC, 9, 0, 0, 0, 0, 0, 0]).is_err());
        assert!(decode_header(&[MAGIC]).is_err());
        assert!(encode_packet(&vec![0; usize::from(u16::MAX) + 1], 1, 0, 0).is_err());
    }
}
