//! RTP video and AAC audio to MPEG-TS, on the cameras' own clocks.
//!
//! Video access units are cut on the RTP timestamp (and closed early by the
//! marker bit) and stamped from the 90 kHz RTP clock, so the frame rate is
//! the camera's. Output starts at the first keyframe.
//!
//! The downstream remux probes the stream for half a second and fixes its
//! stream set then, so audio must be in the PMT before that. Video is held
//! (at most [`AUDIO_WAIT`] of it) until the audio track is confirmed or ruled
//! out, and the first PMT already names every stream.

use crate::error::BridgeError;

use super::{
    rtp_audio::{AudioTrack, Clock, Probe},
    ts_mux::TsMuxer,
    video::{RtpOutput, RtpVideo, VideoCodec},
};

/// Every PTS is offset by one second so early audio can never go negative.
const PTS_BASE: i64 = 90_000;
/// Longest stretch of video held back while the audio track is undecided.
const AUDIO_WAIT: i64 = 90_000;
/// Held access units and bytes at most, whatever their timestamps say.
const MAX_HELD_UNITS: usize = 64;
const MAX_HELD_BYTES: usize = 16 * 1024 * 1024;
/// Largest video access unit accepted; a 1080p HEVC keyframe is far smaller.
const MAX_UNIT_BYTES: usize = 8 * 1024 * 1024;

struct HeldUnit {
    pts: i64,
    keyframe: bool,
    data: Vec<u8>,
}

pub(super) struct RtpToTs {
    video: RtpVideo,
    muxer: Option<TsMuxer>,
    clock: Clock,
    unit: Vec<u8>,
    unit_timestamp: Option<u32>,
    started: bool,
    last_pts: i64,
    held: Vec<HeldUnit>,
    pub(super) audio: AudioTrack,
}

impl RtpToTs {
    pub(super) fn new(video: RtpVideo) -> Self {
        Self {
            video,
            muxer: None,
            clock: Clock::default(),
            unit: Vec::new(),
            unit_timestamp: None,
            started: false,
            last_pts: PTS_BASE,
            held: Vec::new(),
            audio: AudioTrack::default(),
        }
    }

    /// True once a keyframe of a supported codec has been accepted.
    pub(super) const fn started(&self) -> bool {
        self.started
    }

    pub(super) const fn audio_frames(&self) -> (u64, u64) {
        (self.audio.frames, self.audio.unreadable)
    }

    /// Transport-stream bytes produced by one RTP packet, possibly none.
    pub(super) fn feed(&mut self, packet: &[u8]) -> Result<Vec<u8>, BridgeError> {
        let mut output = Vec::new();
        for item in self.video.feed(packet)? {
            match item {
                RtpOutput::Video {
                    timestamp,
                    marker,
                    data,
                } => {
                    if self
                        .unit_timestamp
                        .is_some_and(|current| current != timestamp)
                    {
                        output.extend(self.flush_unit());
                    }
                    if !data.is_empty() {
                        self.unit_timestamp = Some(timestamp);
                        self.unit.extend_from_slice(&data);
                        if self.unit.len() > MAX_UNIT_BYTES {
                            return Err(BridgeError::Capacity(
                                "RTP video access unit exceeded its byte bound".into(),
                            ));
                        }
                    }
                    if marker {
                        output.extend(self.flush_unit());
                    }
                }
                RtpOutput::Other {
                    payload_type,
                    timestamp,
                    payload,
                } => output.extend(self.audio_packet(payload_type, timestamp, &payload)),
            }
        }
        Ok(output)
    }

    /// The access unit still open when the session ends, and any held video.
    pub(super) fn finish(&mut self) -> Vec<u8> {
        let mut output = self.flush_unit();
        self.audio.give_up();
        output.extend(self.release_held(Vec::new()));
        output
    }

    fn flush_unit(&mut self) -> Vec<u8> {
        let Some(timestamp) = self.unit_timestamp.take() else {
            return Vec::new();
        };
        let data = std::mem::take(&mut self.unit);
        let Some(codec) = self.video.codec() else {
            return Vec::new();
        };
        let keyframe = is_keyframe(codec, &data);
        if !self.started && !keyframe {
            return Vec::new();
        }
        if !self.started {
            // The first keyframe defines time zero of the video clock.
            self.clock = Clock::default();
        }
        let pts = PTS_BASE + self.clock.advance(timestamp);
        self.started = true;
        self.last_pts = pts;
        self.held.push(HeldUnit {
            pts,
            keyframe,
            data,
        });
        let held_bytes: usize = self.held.iter().map(|unit| unit.data.len()).sum();
        if self.audio.probe() == Probe::Undecided
            && (pts - self.held[0].pts >= AUDIO_WAIT
                || self.held.len() >= MAX_HELD_UNITS
                || held_bytes >= MAX_HELD_BYTES)
        {
            self.audio.give_up();
        }
        self.release_held(Vec::new())
    }

    /// Writes held video once the audio question is settled, interleaved by
    /// PTS with `audio` (the frames that confirmed the track) so a reader that
    /// probes only the stream's first moments still meets both streams.
    fn release_held(&mut self, audio: Vec<(i64, Vec<u8>)>) -> Vec<u8> {
        let Some(codec) = self.video.codec() else {
            return Vec::new();
        };
        if self.audio.probe() == Probe::Undecided || !self.started {
            return Vec::new();
        }
        let with_audio = self.audio.probe() == Probe::Confirmed;
        let muxer = self.muxer.get_or_insert_with(|| {
            let mut muxer = TsMuxer::new(codec);
            if with_audio {
                muxer.enable_audio();
            }
            muxer
        });
        let mut frames = audio.into_iter().peekable();
        let mut output = Vec::new();
        for unit in self.held.drain(..) {
            while let Some((pts, frame)) = frames.next_if(|(pts, _)| *pts < unit.pts) {
                output.extend(muxer.audio(non_negative(pts), &frame));
            }
            output.extend(muxer.video(non_negative(unit.pts), unit.keyframe, &unit.data));
        }
        for (pts, frame) in frames {
            output.extend(muxer.audio(non_negative(pts), &frame));
        }
        output
    }

    fn audio_packet(&mut self, payload_type: u8, timestamp: u32, payload: &[u8]) -> Vec<u8> {
        let before = self.audio.probe();
        let probe = self
            .audio
            .detect(payload_type, timestamp, payload, self.last_pts);
        match (before, probe) {
            (_, Probe::Undecided) => return Vec::new(),
            // This packet confirmed the track and is among the drained ones.
            // Before the first keyframe there is no video timeline to anchor
            // to: drop the confirming frames and leave the anchor unset.
            (Probe::Undecided, Probe::Confirmed) if !self.started => {
                self.audio.discard_pending();
                return Vec::new();
            }
            (Probe::Undecided, Probe::Confirmed) => {
                let frames = self.audio.drain_pending();
                return self.release_held(frames);
            }
            _ => {}
        }
        let mut output = self.release_held(Vec::new());
        if !self.started {
            return output;
        }
        let frames = self
            .audio
            .frames(payload_type, timestamp, payload, self.last_pts);
        if let Some(muxer) = self.muxer.as_mut() {
            for (pts, frame) in frames {
                output.extend(muxer.audio(non_negative(pts), &frame));
            }
        }
        output
    }
}

fn non_negative(pts: i64) -> u64 {
    u64::try_from(pts).unwrap_or(0)
}

/// IDR for H.264, any IRAP picture for HEVC.
fn is_keyframe(codec: VideoCodec, unit: &[u8]) -> bool {
    unit.windows(5)
        .filter(|window| window[..4] == [0, 0, 0, 1])
        .any(|window| match codec {
            VideoCodec::H264 => window[4] & 0x1f == 5,
            VideoCodec::Hevc => matches!((window[4] >> 1) & 0x3f, 16..=21),
        })
}

#[cfg(test)]
#[path = "rtp_ts_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "rtp_ts_hold_tests.rs"]
mod hold_tests;

#[cfg(test)]
#[path = "rtp_ts_ffmpeg_tests.rs"]
mod ffmpeg_tests;
