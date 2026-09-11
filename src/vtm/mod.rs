mod framing;
mod protobuf;
mod session;

pub use framing::{VtmPacket, decode_header, encode_packet};
pub use protobuf::{StreamInfo, build_keepalive, build_stream_info, parse_stream_info};
pub use session::{VtmSession, VtmUrlRequest, build_vtm_url};

pub const CHANNEL_MESSAGE: u8 = 0x00;
pub const CHANNEL_STREAM: u8 = 0x01;
pub const CHANNEL_ENCRYPTED_MESSAGE: u8 = 0x0a;
pub const CHANNEL_ENCRYPTED_STREAM: u8 = 0x0b;
pub const KEEPALIVE_REQUEST: u16 = 0x132;
pub const KEEPALIVE_RESPONSE: u16 = 0x133;
pub const STREAM_INFO_REQUEST: u16 = 0x13b;
pub const STREAM_INFO_RESPONSE: u16 = 0x13c;
