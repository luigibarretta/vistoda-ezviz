//! VTM `ysproto://` stream URL construction and endpoint parsing.

use std::{collections::BTreeMap, net::Ipv6Addr, str::FromStr};

use url::{Url, form_urlencoded};

use crate::error::BridgeError;

#[must_use]
pub fn build_vtm_url(request: &VtmUrlRequest<'_>) -> String {
    let query = request.biz_url.split_once('?').map_or_else(
        || request.biz_url.trim_start_matches('?'),
        |(_, query)| query,
    );
    let mut params: BTreeMap<String, String> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    for (key, value) in [
        ("dev", request.serial.to_owned()),
        ("chn", request.channel.to_string()),
        (
            "stream",
            if request.substream {
                "2".into()
            } else {
                "1".into()
            },
        ),
        ("cln", "9".into()),
        ("isp", "0".into()),
        ("auth", "1".into()),
        ("ssn", request.token.to_owned()),
        ("vip", "0".into()),
        ("timestamp", request.timestamp_ms.to_string()),
    ] {
        params.insert(key.into(), value);
    }
    let host = if Ipv6Addr::from_str(request.host).is_ok() {
        format!("[{}]", request.host)
    } else {
        request.host.to_owned()
    };
    let query: String = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params)
        .finish();
    format!("ysproto://{host}:{}/live?{query}", request.port)
}

pub struct VtmUrlRequest<'a> {
    pub host: &'a str,
    pub port: u16,
    pub serial: &'a str,
    pub channel: u16,
    pub substream: bool,
    pub biz_url: &'a str,
    pub token: &'a str,
    pub timestamp_ms: i64,
}

pub(super) fn endpoint(value: &str) -> Result<(String, u16), BridgeError> {
    let parsed = Url::parse(value).map_err(|_| BridgeError::Upstream("invalid VTM URL".into()))?;
    if parsed.scheme() != "ysproto" {
        return Err(BridgeError::Upstream("invalid VTM URL scheme".into()));
    }
    Ok((
        parsed
            .host_str()
            .ok_or_else(|| BridgeError::Upstream("VTM URL omitted host".into()))?
            .to_owned(),
        parsed
            .port()
            .ok_or_else(|| BridgeError::Upstream("VTM URL omitted port".into()))?,
    ))
}

#[cfg(test)]
mod tests {
    use super::{VtmUrlRequest, build_vtm_url};

    #[test]
    fn url_targets_exact_channel_and_only_changes_stream_profile() {
        let url = build_vtm_url(&VtmUrlRequest {
            host: "2001:db8::1",
            port: 8554,
            serial: "SERIAL",
            channel: 7,
            substream: true,
            biz_url: "?source=1&substream=1&stream=1",
            token: "secret-token",
            timestamp_ms: 42,
        });
        assert!(url.starts_with("ysproto://[2001:db8::1]:8554/live?"));
        let parsed = url::Url::parse(&url).unwrap_or_else(|error| panic!("{error}"));
        let query: std::collections::BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
        assert_eq!(query.get("chn").map(String::as_str), Some("7"));
        assert_eq!(query.get("stream").map(String::as_str), Some("2"));
        assert_eq!(query.get("substream").map(String::as_str), Some("1"));
        assert_eq!(query.get("ssn").map(String::as_str), Some("secret-token"));
    }
}
