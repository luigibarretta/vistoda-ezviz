use std::collections::BTreeMap;

use crate::error::BridgeError;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StreamInfo {
    pub result: Option<u64>,
    pub stream_session: Option<String>,
    pub redirect_key: Option<String>,
    pub redirect_url: Option<String>,
}

#[must_use]
pub fn build_stream_info(url: &str, redirect_key: Option<&str>) -> Vec<u8> {
    let version = "v3.6.3.20221124";
    let mut output = proto_string(1, url);
    if let Some(key) = redirect_key {
        output.extend(proto_string(2, key));
    }
    output.extend(proto_string(3, version));
    output.extend(proto_varint(4, 0));
    output.extend(proto_string(6, version));
    output
}

#[must_use]
pub fn build_keepalive(stream_session: &str) -> Vec<u8> {
    proto_string(1, stream_session)
}

pub fn parse_stream_info(data: &[u8]) -> Result<StreamInfo, BridgeError> {
    let fields = read_fields(data)?;
    Ok(StreamInfo {
        result: last_int(&fields, 1),
        stream_session: last_string(&fields, 4),
        redirect_key: last_string(&fields, 5),
        redirect_url: last_string(&fields, 7),
    })
}

#[derive(Clone, Debug)]
enum ProtoValue {
    Integer(u64),
    Bytes(Vec<u8>),
}

fn read_fields(data: &[u8]) -> Result<BTreeMap<u64, Vec<ProtoValue>>, BridgeError> {
    let mut fields = BTreeMap::<u64, Vec<ProtoValue>>::new();
    let mut position = 0;
    while position < data.len() {
        let (key, next) = read_varint(data, position)?;
        position = next;
        let field = key >> 3;
        match key & 7 {
            0 => {
                let (value, next) = read_varint(data, position)?;
                position = next;
                fields
                    .entry(field)
                    .or_default()
                    .push(ProtoValue::Integer(value));
            }
            2 => {
                let (length, next) = read_varint(data, position)?;
                position = next;
                let length = usize::try_from(length)
                    .map_err(|_| BridgeError::Upstream("protobuf field is too large".into()))?;
                let end = position.saturating_add(length);
                if end > data.len() {
                    return Err(BridgeError::Upstream(
                        "protobuf field exceeds payload".into(),
                    ));
                }
                fields
                    .entry(field)
                    .or_default()
                    .push(ProtoValue::Bytes(data[position..end].to_vec()));
                position = end;
            }
            _ => {
                return Err(BridgeError::Upstream(
                    "unsupported protobuf wire type".into(),
                ));
            }
        }
    }
    Ok(fields)
}

fn proto_string(field: u64, value: &str) -> Vec<u8> {
    let mut output = encode_varint((field << 3) | 2);
    output.extend(encode_varint(value.len() as u64));
    output.extend_from_slice(value.as_bytes());
    output
}

fn proto_varint(field: u64, value: u64) -> Vec<u8> {
    let mut output = encode_varint(field << 3);
    output.extend(encode_varint(value));
    output
}

fn encode_varint(mut value: u64) -> Vec<u8> {
    let mut output = Vec::new();
    loop {
        let byte = (value & 0x7f) as u8;
        value >>= 7;
        output.push(if value == 0 { byte } else { byte | 0x80 });
        if value == 0 {
            return output;
        }
    }
}

fn read_varint(data: &[u8], mut position: usize) -> Result<(u64, usize), BridgeError> {
    let mut shift = 0;
    let mut value = 0_u64;
    while position < data.len() && shift < 64 {
        let byte = data[position];
        position += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok((value, position));
        }
        shift += 7;
    }
    Err(BridgeError::Upstream("malformed protobuf varint".into()))
}

fn last_int(fields: &BTreeMap<u64, Vec<ProtoValue>>, field: u64) -> Option<u64> {
    match fields.get(&field)?.last()? {
        ProtoValue::Integer(value) => Some(*value),
        ProtoValue::Bytes(_) => None,
    }
}

fn last_string(fields: &BTreeMap<u64, Vec<ProtoValue>>, field: u64) -> Option<String> {
    match fields.get(&field)?.last()? {
        ProtoValue::Bytes(value) => String::from_utf8(value.clone()).ok(),
        ProtoValue::Integer(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_request_matches_python_golden_vector() {
        assert_eq!(
            hex::encode(build_stream_info("ysproto://host/live", None)),
            "0a13797370726f746f3a2f2f686f73742f6c6976651a0f76332e362e332e32303232313132342000320f76332e362e332e3230323231313234"
        );
    }

    #[test]
    fn malformed_or_unsupported_fields_are_rejected() {
        assert!(parse_stream_info(&[0x08, 0x80]).is_err());
        assert!(parse_stream_info(&[0x0a, 0x05, b'a']).is_err());
        assert!(parse_stream_info(&[0x0d, 0, 0, 0, 0]).is_err());
    }

    proptest::proptest! {
        #[test]
        fn varints_roundtrip(value in 0_u64..u64::MAX) {
            let bytes = encode_varint(value);
            let (decoded, used) = read_varint(&bytes, 0).unwrap_or_else(|error| panic!("{error}"));
            proptest::prop_assert_eq!((decoded, used), (value, bytes.len()));
        }
    }
}
