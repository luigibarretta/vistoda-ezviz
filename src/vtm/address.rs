//! VTM `ysproto://` stream URL construction and endpoint parsing.

use std::{collections::BTreeMap, net::Ipv6Addr, str::FromStr};

use url::{Url, form_urlencoded};

use crate::error::BridgeError;

#[must_use]
pub fn build_vtm_url(request: &VtmUrlRequest<'_>) -> String {
    let stream = if request.substream { "2" } else { "1" };
    let params = query_params(
        request,
        [
            ("dev", request.serial.to_owned()),
            ("chn", request.channel.to_string()),
            ("stream", stream.into()),
            ("cln", "9".into()),
            ("isp", "0".into()),
            ("auth", "1".into()),
            ("ssn", request.token.to_owned()),
            ("vip", "0".into()),
            ("timestamp", request.timestamp_ms.to_string()),
        ],
    );
    format!(
        "ysproto://{}:{}/live?{params}",
        host(request.host),
        request.port
    )
}

/// microSD playback of one camera-local interval.
///
/// Bounds are `yyyyMMddTHHmmssZ` in the camera's wall clock despite the `Z`
/// (the app's `CasUtils.convertCasTime`; a UTC value is answered with result
/// 5402). Parameter set of pyEzvizApi PR #266, client type 3.
#[must_use]
pub fn build_vtm_playback_url(request: &VtmUrlRequest<'_>, begin: &str, end: &str) -> String {
    let params = query_params(
        request,
        [
            ("dev", request.serial.to_owned()),
            ("chn", request.channel.to_string()),
            ("stream", "1".into()),
            ("begin", begin.to_owned()),
            ("end", end.to_owned()),
            ("serial", request.serial.to_owned()),
            ("cln", "3".into()),
            ("isp", "0".into()),
            ("auth", "1".into()),
            ("ssn", request.token.to_owned()),
            ("rnd", (request.timestamp_ms & 0x7fff_ffff).to_string()),
            ("timestamp", request.timestamp_ms.to_string()),
            ("etp", "1".into()),
        ],
    );
    format!(
        "ysproto://{}:{}/playback?{params}",
        host(request.host),
        request.port
    )
}

/// The `streamBizUrl` parameters overlaid with the request's own.
fn query_params<const N: usize>(request: &VtmUrlRequest<'_>, own: [(&str, String); N]) -> String {
    let query = request.biz_url.split_once('?').map_or_else(
        || request.biz_url.trim_start_matches('?'),
        |(_, query)| query,
    );
    let mut params: BTreeMap<String, String> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    for (key, value) in own {
        params.insert(key.into(), value);
    }
    form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params)
        .finish()
}

fn host(value: &str) -> String {
    if Ipv6Addr::from_str(value).is_ok() {
        format!("[{value}]")
    } else {
        value.to_owned()
    }
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
    use super::{VtmUrlRequest, build_vtm_playback_url, build_vtm_url};

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

    #[test]
    fn playback_url_carries_the_interval_and_the_playback_parameter_set() {
        let request = VtmUrlRequest {
            host: "vtm.example",
            port: 8554,
            serial: "SERIAL",
            channel: 1,
            substream: true,
            biz_url: "?ssn=biz&stream=2&source=1",
            token: "vtdu-token",
            timestamp_ms: 0x1_2345_6789,
        };
        let url = build_vtm_playback_url(&request, "20261024T061500Z", "20261024T061530Z");
        assert!(url.starts_with("ysproto://vtm.example:8554/playback?"));
        let parsed = url::Url::parse(&url).unwrap_or_else(|error| panic!("{error}"));
        let query: std::collections::BTreeMap<_, _> = parsed.query_pairs().into_owned().collect();
        for (key, value) in [
            ("dev", "SERIAL"),
            ("serial", "SERIAL"),
            ("chn", "1"),
            ("stream", "1"),
            ("begin", "20261024T061500Z"),
            ("end", "20261024T061530Z"),
            ("cln", "3"),
            ("isp", "0"),
            ("auth", "1"),
            ("ssn", "vtdu-token"),
            ("etp", "1"),
            ("source", "1"),
            ("rnd", "591751049"),
            ("timestamp", "4886718345"),
        ] {
            assert_eq!(query.get(key).map(String::as_str), Some(value), "{key}");
        }
        assert!(!query.contains_key("vip"), "vip belongs to live only");
    }
}
