//! Receives real RTP from `FFmpeg` and writes the muxed MPEG-TS for an external
//! `FFmpeg` check. Run by `scripts/verify-rtp-ffmpeg.sh`, never by default.

use std::time::Duration;

use tokio::{net::UdpSocket, time::timeout};

use super::*;

const VIDEO_PORT: u16 = 5004;
const AUDIO_PORT: u16 = 5006;
/// `FFmpeg` is done once neither socket has received anything for this long.
const IDLE: Duration = Duration::from_secs(3);

#[tokio::test]
#[ignore = "needs FFmpeg sending RTP; run scripts/verify-rtp-ffmpeg.sh"]
async fn real_ffmpeg_rtp_becomes_mpeg_ts() {
    let output = std::env::var("VISTODA_TS_OUTPUT").unwrap_or_else(|error| panic!("{error}"));
    let video = UdpSocket::bind(("127.0.0.1", VIDEO_PORT))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let audio = UdpSocket::bind(("127.0.0.1", AUDIO_PORT))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    println!("listening");
    let mut stream = RtpToTs::new(RtpVideo::clear());
    let mut ts = Vec::new();
    let (mut video_packets, mut audio_packets) = (0_u32, 0_u32);
    let mut video_buffer = vec![0_u8; 65_536];
    let mut audio_buffer = vec![0_u8; 65_536];
    // The first packet may take as long as FFmpeg's container start.
    let mut wait = Duration::from_secs(120);
    loop {
        let received = timeout(wait, async {
            tokio::select! {
                result = video.recv(&mut video_buffer) => (true, result),
                result = audio.recv(&mut audio_buffer) => (false, result),
            }
        })
        .await;
        let Ok((is_video, result)) = received else {
            break;
        };
        let length = result.unwrap_or_else(|error| panic!("{error}"));
        let packet = if is_video {
            video_packets += 1;
            &video_buffer[..length]
        } else {
            audio_packets += 1;
            &audio_buffer[..length]
        };
        ts.extend(
            stream
                .feed(packet)
                .unwrap_or_else(|error| panic!("{error}")),
        );
        wait = IDLE;
    }
    ts.extend(stream.finish());
    println!(
        "video_packets={video_packets} audio_packets={audio_packets} audio={:?} ts_bytes={}",
        stream.audio_frames(),
        ts.len()
    );
    assert!(stream.started(), "no keyframe was muxed");
    assert!(stream.audio_frames().0 > 0, "no audio was muxed");
    std::fs::write(output, ts).unwrap_or_else(|error| panic!("{error}"));
}
