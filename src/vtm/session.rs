use std::time::Duration;

use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{Instant, timeout},
};
use tokio_util::sync::CancellationToken;

use crate::{
    error::BridgeError,
    vtm::{
        CHANNEL_MESSAGE, CHANNEL_STREAM, CHANNEL_STREAM_ALT, KEEPALIVE_REQUEST, KEEPALIVE_RESPONSE,
        STREAM_INFO_REQUEST, STREAM_INFO_RESPONSE, StreamInfo, VtmPacket, address::endpoint,
        build_keepalive, build_playback_stream_info, build_stream_info, decode_header,
        encode_packet, framing::HEADER_SIZE,
    },
};

pub struct VtmSession {
    stream: TcpStream,
    stream_url: String,
    sequence: u16,
    info: StreamInfo,
    timeout: Duration,
    last_keepalive: Instant,
    /// Last media payload, or the session start. Keepalive replies keep the
    /// socket busy without video, so they must not count as liveness.
    last_media: Instant,
    /// microSD playback: the relay ends a finished interval by closing.
    playback: bool,
    media_seen: bool,
}

impl VtmSession {
    pub async fn connect(
        stream_url: String,
        timeout_duration: Duration,
    ) -> Result<Self, BridgeError> {
        Self::open(stream_url, timeout_duration, false).await
    }

    /// A `/playback?` session; it ends when the relay closes after media.
    pub async fn connect_playback(
        stream_url: String,
        timeout_duration: Duration,
    ) -> Result<Self, BridgeError> {
        Self::open(stream_url, timeout_duration, true).await
    }

    async fn open(
        stream_url: String,
        timeout_duration: Duration,
        playback: bool,
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
                last_keepalive: Instant::now(),
                last_media: Instant::now(),
                playback,
                media_seen: false,
            };
            session.start(redirect_key.as_deref()).await?;
            if let Some(result) = session.info.result.filter(|result| *result != 0) {
                match result {
                    // 5404 offline; 5405 the device did not answer (asleep).
                    5404 | 5405 => return Err(BridgeError::CameraOffline),
                    // No microSD recording in the requested interval.
                    5402 if playback => return Err(BridgeError::SdNoRecording),
                    _ => {}
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
            session.last_media = Instant::now();
            return Ok(session);
        }
        Err(BridgeError::Upstream("too many VTM redirects".into()))
    }

    async fn start(&mut self, redirect_key: Option<&str>) -> Result<(), BridgeError> {
        let body = if self.playback {
            build_playback_stream_info(&self.stream_url, redirect_key)
        } else {
            build_stream_info(&self.stream_url, redirect_key)
        };
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
        loop {
            let Some(payload) = self.next_payload(cancel, false).await? else {
                return Ok(());
            };
            if !payload.is_empty() && !consume(payload) {
                return Ok(());
            }
        }
    }

    /// The next media payload, keeping the session alive. Encrypted media only
    /// reaches the opt-in decryptor; encrypted control messages fail closed.
    pub async fn next_payload(
        &mut self,
        cancel: &CancellationToken,
        allow_encrypted_stream: bool,
    ) -> Result<Option<Vec<u8>>, BridgeError> {
        loop {
            if cancel.is_cancelled() {
                return Ok(None);
            }
            if self.last_keepalive.elapsed() >= Duration::from_secs(5) {
                match self.keepalive(KEEPALIVE_REQUEST).await {
                    Err(BridgeError::Io(error)) if self.finished_playback(&error) => {
                        return Ok(None);
                    }
                    other => other?,
                }
                self.last_keepalive = Instant::now();
            }
            let media_deadline = self.last_media + self.timeout;
            // Biased: the media deadline beats the per-read timeout it ties with.
            let packet = tokio::select! {
                biased;
                () = cancel.cancelled() => return Ok(None),
                () = tokio::time::sleep_until(media_deadline) => {
                    // A finished interval may go quiet instead of closing;
                    // otherwise the camera slept or never woke.
                    if self.playback && self.media_seen {
                        return Ok(None);
                    }
                    tracing::warn!("VTM session carried no media within its timeout");
                    return Err(BridgeError::CameraOffline);
                }
                packet = self.read_packet() => match packet {
                    Err(BridgeError::Io(error)) if self.finished_playback(&error) => {
                        return Ok(None);
                    }
                    other => other?,
                },
            };
            if packet.message_code == KEEPALIVE_REQUEST {
                self.keepalive(KEEPALIVE_RESPONSE).await?;
                self.last_keepalive = Instant::now();
                continue;
            }
            if matches!(packet.channel, CHANNEL_STREAM | CHANNEL_STREAM_ALT)
                || (allow_encrypted_stream
                    && packet.channel == crate::vtm::CHANNEL_ENCRYPTED_STREAM)
            {
                // An empty stream packet must not keep a sleeping camera alive.
                if !packet.body.is_empty() {
                    self.last_media = Instant::now();
                    self.media_seen = true;
                }
                return Ok(Some(packet.body));
            }
            if packet.encrypted() {
                return Err(BridgeError::Upstream(
                    "encrypted VTM control packet is unsupported".into(),
                ));
            }
        }
    }

    /// A playback relay closing the socket after media ends the interval.
    fn finished_playback(&self, error: &std::io::Error) -> bool {
        use std::io::ErrorKind;
        self.playback
            && self.media_seen
            && matches!(
                error.kind(),
                ErrorKind::UnexpectedEof | ErrorKind::ConnectionReset | ErrorKind::BrokenPipe
            )
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
