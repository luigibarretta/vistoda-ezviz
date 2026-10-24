use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::{
    error::BridgeError,
    transport::{ChunkConsumer, rtp_ts::RtpToTs, video::RtpVideo},
    vtm::VtmSession,
};

/// Serves an RTP camera as MPEG-TS: depacketized (and decrypted when the
/// [`RtpVideo`] holds a key) and muxed in-process on the RTP timestamps,
/// with the camera's AAC audio when it sends any. `first` is a payload the
/// caller already read to choose this path.
pub(super) async fn copy_as_mpeg_ts(
    mut session: VtmSession,
    video: RtpVideo,
    first: Option<Vec<u8>>,
    encrypted_channel: bool,
    cancel: CancellationToken,
    output: Arc<dyn ChunkConsumer>,
) -> Result<(), BridgeError> {
    let mut stream = RtpToTs::new(video);
    let mut first = first;
    loop {
        let packet = match first.take() {
            Some(packet) => packet,
            None => match session.next_payload(&cancel, encrypted_channel).await? {
                Some(packet) => packet,
                None => break,
            },
        };
        if packet.is_empty() {
            continue;
        }
        let chunk = stream.feed(&packet)?;
        if !chunk.is_empty() && !output.consume(chunk) {
            return Ok(());
        }
    }
    let (frames, unreadable) = stream.audio_frames();
    if unreadable > 0 {
        tracing::warn!(frames, unreadable, "RTP audio payloads could not be read");
    }
    if !stream.started() {
        return if cancel.is_cancelled() {
            Ok(())
        } else {
            Err(BridgeError::Upstream(
                "RTP stream ended before a keyframe of a supported codec".into(),
            ))
        };
    }
    let tail = stream.finish();
    if !tail.is_empty() {
        output.consume(tail);
    }
    Ok(())
}
