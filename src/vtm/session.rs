use std::{collections::BTreeMap, net::Ipv6Addr, str::FromStr, time::Duration};

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;
use url::{Url, form_urlencoded};

use crate::{
    error::BridgeError,
    vtm::{
        CHANNEL_MESSAGE, CHANNEL_STREAM, KEEPALIVE_REQUEST, KEEPALIVE_RESPONSE,
        STREAM_INFO_REQUEST, STREAM_INFO_RESPONSE, StreamInfo, VtmPacket, build_keepalive,
        build_stream_info, decode_header, encode_packet, framing::HEADER_SIZE,
    },
};

pub struct VtmSession {
    stream: TcpStream,
    stream_url: String,
    sequence: u16,
    info: StreamInfo,
    timeout: Duration,
}

impl VtmSession {
    pub async fn connect(
        stream_url: String,
        timeout_duration: Duration,
    ) -> Result<Self, BridgeError> {
        let mut current_url = stream_url;
        let mut redirect_key = None;
        for _ in 0..=3 {
            let (host, port) = endpoint(&current_url)?;
            let stream = timeout(timeout_duration, TcpStream::connect((host.as_str(), port)))
                .await
                .map_err(|_| BridgeError::Upstream("VTM connection timed out".into()))??;
            let mut session = Self {
                stream,
                stream_url: current_url.clone(),
                sequence: 0,
                info: StreamInfo::default(),
                timeout: timeout_duration,
            };
            session.start(redirect_key.as_deref()).await?;
            if let Some(result) = session.info.result.filter(|result| *result != 0) {
                if result == 5404 {
                    return Err(BridgeError::CameraOffline);
                }
                let Some(url) = session.info.redirect_url.clone() else {
                    return Err(BridgeError::Upstream(format!(
                        "VTM stream request was rejected with result {result}"
                    )));
                };
                let Some(key) = session.info.redirect_key.clone() else {
                    return Err(BridgeError::Upstream("VTM redirect omitted its key".into()));
                };
                current_url = url;
                redirect_key = Some(key);
                continue;
            }
            return Ok(session);
        }
        Err(BridgeError::Upstream("too many VTM redirects".into()))
    }

    async fn start(&mut self, redirect_key: Option<&str>) -> Result<(), BridgeError> {
        let body = build_stream_info(&self.stream_url, redirect_key);
        self.send(CHANNEL_MESSAGE, STREAM_INFO_REQUEST, &body)
            .await?;
        for _ in 0..20 {
            let packet = self.read_packet().await?;
            if packet.message_code == STREAM_INFO_RESPONSE {
                self.info = crate::vtm::parse_stream_info(&packet.body)?;
                return Ok(());
            }
        }
        Err(BridgeError::Upstream(
            "timed out waiting for VTM stream info".into(),
        ))
    }

    pub async fn copy_payloads<F>(
        &mut self,
        cancel: &CancellationToken,
        mut consume: F,
    ) -> Result<(), BridgeError>
    where
        F: FnMut(Vec<u8>) -> bool,
    {
        let mut last_keepalive = Instant::now();
        loop {
            if cancel.is_cancelled() {
                return Ok(());
            }
            if last_keepalive.elapsed() >= Duration::from_secs(5) {
                self.keepalive(KEEPALIVE_REQUEST).await?;
                last_keepalive = Instant::now();
            }
            let packet = tokio::select! {
                () = cancel.cancelled() => return Ok(()),
                packet = self.read_packet() => packet?,
            };
            if packet.message_code == KEEPALIVE_REQUEST {
                self.keepalive(KEEPALIVE_RESPONSE).await?;
                last_keepalive = Instant::now();
            } else if packet.channel == CHANNEL_STREAM {
                if !packet.body.is_empty() && !consume(packet.body) {
                    return Ok(());
                }
            } else if packet.encrypted() {
                return Err(BridgeError::Upstream(
                    "encrypted VTM stream packets are unsupported".into(),
                ));
            }
        }
    }

    async fn keepalive(&mut self, code: u16) -> Result<(), BridgeError> {
        let session =
            self.info.stream_session.clone().ok_or_else(|| {
                BridgeError::Upstream("VTM keepalive has no stream session".into())
            })?;
        self.send(CHANNEL_MESSAGE, code, &build_keepalive(&session))
            .await
    }

    async fn send(&mut self, channel: u8, code: u16, body: &[u8]) -> Result<(), BridgeError> {
        let packet = encode_packet(body, channel, self.sequence, code)?;
        self.sequence = self.sequence.wrapping_add(1);
        timeout(self.timeout, self.stream.write_all(&packet))
            .await
            .map_err(|_| BridgeError::Upstream("VTM write timed out".into()))??;
        Ok(())
    }

    async fn read_packet(&mut self) -> Result<VtmPacket, BridgeError> {
        let mut header = [0_u8; HEADER_SIZE];
        timeout(self.timeout, self.stream.read_exact(&mut header))
            .await
            .map_err(|_| BridgeError::Upstream("VTM read timed out".into()))??;
        let (channel, length, sequence, message_code) = decode_header(&header)?;
        let mut body = vec![0; length];
        timeout(self.timeout, self.stream.read_exact(&mut body))
            .await
            .map_err(|_| BridgeError::Upstream("VTM body read timed out".into()))??;
        Ok(VtmPacket {
            channel,
            sequence,
            message_code,
            body,
        })
    }
}

#[must_use]
pub fn build_vtm_url(
    host: &str,
    port: u16,
    serial: &str,
    biz_url: &str,
    token: &str,
    timestamp_ms: i64,
) -> String {
    let query = biz_url
        .split_once('?')
        .map_or_else(|| biz_url.trim_start_matches('?'), |(_, query)| query);
    let mut params: BTreeMap<String, String> = form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    for (key, value) in [
        ("dev", serial.to_owned()),
        ("chn", "1".into()),
        ("stream", "1".into()),
        ("cln", "9".into()),
        ("isp", "0".into()),
        ("auth", "1".into()),
        ("ssn", token.to_owned()),
        ("vip", "0".into()),
        ("timestamp", timestamp_ms.to_string()),
    ] {
        params.insert(key.into(), value);
    }
    let host = if Ipv6Addr::from_str(host).is_ok() {
        format!("[{host}]")
    } else {
        host.to_owned()
    };
    let query: String = form_urlencoded::Serializer::new(String::new())
        .extend_pairs(params)
        .finish();
    format!("ysproto://{host}:{port}/live?{query}")
}

fn endpoint(value: &str) -> Result<(String, u16), BridgeError> {
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
