//! The audio half of an RTP session: which payload type is AAC, and the
//! ADTS frames it carries stamped on a 90 kHz timeline shared with video.
//!
//! The audio RTP clock has an unrelated random base, so the track is anchored
//! to the video PTS current when its first frame is written and then advances
//! on its own timestamps. A payload type becomes audio only after several
//! consecutive AAC frames, in packets that read as AAC throughout (a camera
//! sends one frame per packet, `FFmpeg` several); anything else is never muxed.

use super::aac_rtp::{FRAME_SAMPLES, SAMPLE_RATE, access_units, adts_frame, channels};

/// Consecutive AAC frames needed before a payload type is audio.
const CONFIRMATIONS: usize = 8;
/// One AAC frame on the 90 kHz timeline: 1024 samples at 16 kHz.
const FRAME_TICKS: i64 = 5_760;
/// Non-video packets examined before audio detection gives up.
const PROBE_LIMIT: u32 = 400;

/// Extends a wrapping 32-bit RTP timestamp to a signed running offset.
#[derive(Default)]
pub(super) struct Clock {
    last: Option<u32>,
    elapsed: i64,
}

impl Clock {
    pub(super) fn advance(&mut self, timestamp: u32) -> i64 {
        if let Some(last) = self.last {
            let step = i32::from_ne_bytes(timestamp.wrapping_sub(last).to_ne_bytes());
            self.elapsed += i64::from(step);
        }
        self.last = Some(timestamp);
        self.elapsed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Probe {
    Undecided,
    Confirmed,
    Absent,
}

#[derive(Default)]
pub(super) struct AudioTrack {
    pub(super) payload_type: Option<u8>,
    candidate: Option<(u8, usize)>,
    /// The candidate's packets with the video PTS current at their arrival;
    /// written once confirmed, so audio starts with the stream, not after it.
    pending: Vec<(u32, Vec<u8>, i64)>,
    pub(super) probed: u32,
    pub(super) gave_up: bool,
    channels: u8,
    clock: Clock,
    anchor: Option<i64>,
    pub(super) frames: u64,
    pub(super) unreadable: u64,
}

impl AudioTrack {
    pub(super) const fn probe(&self) -> Probe {
        if self.payload_type.is_some() {
            Probe::Confirmed
        } else if self.gave_up {
            Probe::Absent
        } else {
            Probe::Undecided
        }
    }

    /// Feeds one non-video packet to detection; a no-op once decided.
    pub(super) fn detect(
        &mut self,
        payload_type: u8,
        timestamp: u32,
        payload: &[u8],
        video_pts: i64,
    ) -> Probe {
        if self.probe() != Probe::Undecided {
            return self.probe();
        }
        self.probed += 1;
        // Every frame must open with a channel element, not just the first.
        let valid = access_units(payload).and_then(|units| {
            let first = units.first().and_then(|unit| channels(unit))?;
            units
                .iter()
                .all(|unit| channels(unit).is_some())
                .then_some((first, units.len()))
        });
        self.candidate = match (self.candidate, valid) {
            (Some((kind, count)), Some((_, frames))) if kind == payload_type => {
                Some((kind, count + frames))
            }
            (_, Some((_, frames))) => {
                self.pending.clear();
                Some((payload_type, frames))
            }
            (_, None) => {
                self.pending.clear();
                None
            }
        };
        if valid.is_some() {
            self.pending.push((timestamp, payload.to_vec(), video_pts));
        }
        if let Some((kind, count)) = self.candidate
            && count >= CONFIRMATIONS
        {
            self.payload_type = Some(kind);
            self.channels = valid.map_or(1, |(channels, _)| channels);
            tracing::info!(
                payload_type = kind,
                channels = self.channels,
                "RTP AAC audio track found"
            );
        } else if self.probed >= PROBE_LIMIT {
            self.give_up();
        }
        self.probe()
    }

    /// Frames of the packets that confirmed the track, anchored at the video
    /// PTS current when the first of them arrived.
    pub(super) fn drain_pending(&mut self) -> Vec<(i64, Vec<u8>)> {
        let Some(payload_type) = self.payload_type else {
            return Vec::new();
        };
        let pending = std::mem::take(&mut self.pending);
        let mut output = Vec::new();
        for (timestamp, payload, video_pts) in pending {
            output.extend(self.frames(payload_type, timestamp, &payload, video_pts));
        }
        output
    }

    /// Drops the confirming packets without touching the clock or anchor.
    pub(super) fn discard_pending(&mut self) {
        self.pending.clear();
    }

    pub(super) fn give_up(&mut self) {
        self.pending.clear();
        if self.probe() == Probe::Undecided {
            self.gave_up = true;
            tracing::info!("no RTP payload type read as AAC audio; serving video only");
        }
    }

    /// ADTS frames of one packet of the confirmed type, each with its PTS.
    /// `video_pts` anchors the track the first time frames are produced.
    pub(super) fn frames(
        &mut self,
        payload_type: u8,
        timestamp: u32,
        payload: &[u8],
        video_pts: i64,
    ) -> Vec<(i64, Vec<u8>)> {
        if self.payload_type != Some(payload_type) {
            return Vec::new();
        }
        let Some(units) = access_units(payload) else {
            self.unreadable += 1;
            return Vec::new();
        };
        let elapsed = self.clock.advance(timestamp);
        // A packet arrives when its last frame is complete: the first frame
        // of the first packet began that many frames earlier.
        let span = i64::try_from(units.len().saturating_sub(1)).unwrap_or(0) * FRAME_TICKS;
        let anchor = *self.anchor.get_or_insert(video_pts - span);
        let mut output = Vec::with_capacity(units.len());
        for (index, unit) in units.into_iter().enumerate() {
            self.channels = channels(unit).unwrap_or(self.channels);
            let samples = elapsed + i64::try_from(index).unwrap_or(0) * i64::from(FRAME_SAMPLES);
            let pts = anchor + samples * 90_000 / i64::from(SAMPLE_RATE);
            output.push((pts, adts_frame(unit, self.channels)));
            self.frames += 1;
        }
        output
    }
}
