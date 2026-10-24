//! Writes one recording's media to the spool: alignment on the first MPEG-PS
//! pack or MPEG-TS sync, the byte bound, SHA-256 and an atomic rename.

use std::time::{Duration, Instant};

use bytes::Bytes;
use sha2::{Digest, Sha256};
use tokio::{
    fs::{self, OpenOptions},
    io::AsyncWriteExt,
    sync::mpsc,
};

use super::{RecordingManager, model::utc_now};
use crate::error::BridgeError;

const TS_PACKET_BYTES: usize = 188;
const PACK_START: &[u8] = b"\x00\x00\x01\xba";

#[derive(Clone, Copy)]
pub(super) enum RecordingFormat {
    MpegPs,
    MpegTs,
}

impl RecordingFormat {
    pub(super) const fn extension(self) -> &'static str {
        match self {
            Self::MpegPs => "mpegps",
            Self::MpegTs => "ts",
        }
    }

    pub(super) const fn media_type(self) -> &'static str {
        match self {
            Self::MpegPs => "video/mpeg",
            Self::MpegTs => "video/mp2t",
        }
    }
}

/// A finished, renamed and synced media file.
pub(super) struct Written {
    pub(super) started: Instant,
    pub(super) started_at: String,
    pub(super) bytes: u64,
    pub(super) sha256: String,
    pub(super) format: RecordingFormat,
}

impl RecordingManager {
    /// Writes media from `receiver` until it closes or, when given, until
    /// `stop_after` has elapsed since the first aligned byte. A failure
    /// removes the partial file.
    pub(super) async fn write_media(
        &self,
        id: &str,
        receiver: &mut mpsc::Receiver<Bytes>,
        stop_after: Option<Duration>,
    ) -> Result<Written, BridgeError> {
        let partial = self.directory.join(format!(".{id}.partial"));
        let result = self.write_partial(id, &partial, receiver, stop_after).await;
        if result.is_err() {
            let _ignored = fs::remove_file(&partial).await;
        }
        result
    }

    /// Removes the partial file a cancelled `write_media` left behind.
    pub(super) async fn discard_partial(&self, id: &str) {
        let _ignored = fs::remove_file(self.directory.join(format!(".{id}.partial"))).await;
    }

    async fn write_partial(
        &self,
        id: &str,
        partial: &std::path::Path,
        receiver: &mut mpsc::Receiver<Bytes>,
        stop_after: Option<Duration>,
    ) -> Result<Written, BridgeError> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(partial)
            .await?;
        let mut digest = Sha256::new();
        let mut written = 0_u64;
        let mut alignment = Vec::new();
        let mut started = None;
        let mut started_at = None;
        let mut format = None;
        while let Some(source) = receiver.recv().await {
            let chunk = if started.is_none() {
                alignment.extend_from_slice(&source);
                let Some((marker, detected)) = media_alignment(&alignment) else {
                    if alignment.len() > 1024 * 1024 {
                        return Err(BridgeError::Recording(
                            "MPEG-PS or MPEG-TS alignment was not found".into(),
                        ));
                    }
                    continue;
                };
                started = Some(Instant::now());
                started_at = Some(utc_now());
                format = Some(detected);
                alignment.split_off(marker)
            } else {
                source.to_vec()
            };
            let next = written.saturating_add(chunk.len() as u64);
            if next > self.max_bytes {
                return Err(BridgeError::Capacity(
                    "recording reached the configured byte limit".into(),
                ));
            }
            file.write_all(&chunk).await?;
            digest.update(&chunk);
            written = next;
            if started
                .zip(stop_after)
                .is_some_and(|(instant, limit)| instant.elapsed() >= limit)
            {
                break;
            }
        }
        let started = started
            .ok_or_else(|| BridgeError::Recording("recording contained no aligned media".into()))?;
        let format = format
            .ok_or_else(|| BridgeError::Recording("recording format was not identified".into()))?;
        file.sync_all().await?;
        drop(file);
        let final_path = self.directory.join(format!("{id}.{}", format.extension()));
        fs::rename(partial, &final_path).await?;
        std::fs::File::open(&self.directory)?.sync_all()?;
        Ok(Written {
            started,
            started_at: started_at.unwrap_or_else(utc_now),
            bytes: written,
            sha256: hex::encode(digest.finalize()),
            format,
        })
    }
}

fn media_alignment(data: &[u8]) -> Option<(usize, RecordingFormat)> {
    if let Some(offset) = data
        .windows(PACK_START.len())
        .position(|window| window == PACK_START)
    {
        return Some((offset, RecordingFormat::MpegPs));
    }
    (0..data.len().min(TS_PACKET_BYTES)).find_map(|offset| {
        (data.get(offset) == Some(&0x47)
            && data.get(offset + TS_PACKET_BYTES) == Some(&0x47)
            && data.get(offset + TS_PACKET_BYTES * 2) == Some(&0x47))
        .then_some((offset, RecordingFormat::MpegTs))
    })
}

#[cfg(test)]
mod tests {
    use super::{PACK_START, RecordingFormat, TS_PACKET_BYTES, media_alignment};

    #[test]
    fn finds_ts_sync_after_an_untrusted_prefix() {
        let mut data = vec![0xaa; 17 + TS_PACKET_BYTES * 3];
        for index in 0..3 {
            data[17 + index * TS_PACKET_BYTES] = 0x47;
        }
        assert!(matches!(
            media_alignment(&data),
            Some((17, RecordingFormat::MpegTs))
        ));
        data[17 + TS_PACKET_BYTES * 2] = 0;
        assert!(media_alignment(&data).is_none());
        let mut ps = vec![0xaa; 8];
        ps.extend_from_slice(PACK_START);
        assert!(matches!(
            media_alignment(&ps),
            Some((8, RecordingFormat::MpegPs))
        ));
    }
}
