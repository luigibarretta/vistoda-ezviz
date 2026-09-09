use std::{io, process::Stdio, sync::Arc};

use axum::{body::Body, http::StatusCode, response::Response};
use bytes::Bytes;
use tokio::{io::AsyncReadExt, process::Command};

use crate::api::{Runtime, no_store, response};

const FFMPEG_ARGUMENTS: &[&str] = &[
    "-hide_banner",
    "-loglevel",
    "error",
    "-fflags",
    "+genpts",
    "-i",
];
const OUTPUT_ARGUMENTS: &[&str] = &[
    "-map",
    "0:v:0?",
    "-map",
    "0:a:0?",
    "-c:v",
    "copy",
    "-c:a",
    "aac",
    "-avoid_negative_ts",
    "make_zero",
    "-movflags",
    "frag_keyframe+empty_moov+default_base_moof",
    "-f",
    "mp4",
    "pipe:1",
];

pub async fn playback(runtime: Arc<Runtime>, id: &str) -> Result<Response, StatusCode> {
    let path = runtime
        .recordings
        .media_path(id)
        .await
        .ok_or(StatusCode::NOT_FOUND)?;
    let mut child = Command::new(&runtime.config.ffmpeg_path)
        .args(FFMPEG_ARGUMENTS)
        .arg(path)
        .args(OUTPUT_ARGUMENTS)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|_| StatusCode::BAD_GATEWAY)?;
    let mut stdout = child.stdout.take().ok_or(StatusCode::BAD_GATEWAY)?;
    let stream = async_stream::stream! {
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            match stdout.read(&mut buffer).await {
                Ok(0) => break,
                Ok(read) => yield Ok::<Bytes, io::Error>(Bytes::copy_from_slice(&buffer[..read])),
                Err(error) => {
                    yield Err(error);
                    return;
                }
            }
        }
        match child.wait().await {
            Ok(status) if status.success() => {}
            Ok(_) => yield Err(io::Error::other("FFmpeg playback remux failed")),
            Err(error) => yield Err(error),
        }
    };
    let mut result = response(StatusCode::OK, "video/mp4", Body::from_stream(stream));
    no_store(result.headers_mut());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::OUTPUT_ARGUMENTS;

    #[test]
    fn emits_fragmented_browser_mp4_without_reencoding_video() {
        let arguments = OUTPUT_ARGUMENTS.join(" ");
        assert!(arguments.contains("-c:v copy"));
        assert!(arguments.contains("-c:a aac"));
        assert!(arguments.contains("frag_keyframe+empty_moov+default_base_moof"));
        assert!(arguments.ends_with("-f mp4 pipe:1"));
    }
}
