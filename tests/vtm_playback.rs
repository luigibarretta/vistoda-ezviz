//! microSD playback sessions: the request shape, a clean end when the relay
//! closes after the interval, and the vendor result codes.

use std::time::Duration;

use ezviz_vtm_bridge::{
    error::BridgeError,
    vtm::{
        CHANNEL_MESSAGE, CHANNEL_STREAM, CHANNEL_STREAM_ALT, STREAM_INFO_RESPONSE, VtmSession,
        decode_header, encode_packet,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

const TIMEOUT: Duration = Duration::from_secs(2);

/// Accepts one session, returns its stream-info request and answers `result`.
async fn answer(listener: TcpListener, result: &[u8]) -> (TcpStream, Vec<u8>) {
    let (mut socket, _) = listener
        .accept()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let mut header = [0_u8; 8];
    socket
        .read_exact(&mut header)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let (_, length, _, _) = decode_header(&header).unwrap_or_else(|error| panic!("{error}"));
    let mut request = vec![0; length];
    socket
        .read_exact(&mut request)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let mut body = vec![0x08];
    body.extend_from_slice(result);
    body.extend_from_slice(&[0x22, 0x07, b's', b'e', b's', b's', b'i', b'o', b'n']);
    send(&mut socket, CHANNEL_MESSAGE, STREAM_INFO_RESPONSE, &body).await;
    (socket, request)
}

async fn send(socket: &mut TcpStream, channel: u8, code: u16, body: &[u8]) {
    let packet = encode_packet(body, channel, 0, code).unwrap_or_else(|error| panic!("{error}"));
    socket
        .write_all(&packet)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
}

async fn listener() -> (TcpListener, String) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    (listener, address.to_string())
}

#[tokio::test]
async fn playback_asks_type_10_and_ends_cleanly_when_the_relay_closes() {
    let (listener, address) = listener().await;
    let server = tokio::spawn(async move {
        let (mut socket, request) = answer(listener, &[0x00]).await;
        send(&mut socket, CHANNEL_STREAM, 0, b"\x00\x00\x01\xba-one").await;
        send(&mut socket, CHANNEL_STREAM_ALT, 0, b"\x00\x00\x01\xba-two").await;
        // The interval is over: the relay hangs up.
        drop(socket);
        request
    });
    let url = format!("ysproto://{address}/playback?dev=fixture&ssn=fixture");
    let mut session = VtmSession::connect_playback(url, TIMEOUT)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let cancel = CancellationToken::new();
    for expected in [&b"-one"[..], b"-two"] {
        let payload = session
            .next_payload(&cancel, false)
            .await
            .unwrap_or_else(|error| panic!("{error}"))
            .unwrap_or_else(|| panic!("media expected"));
        assert!(payload.ends_with(expected));
    }
    let end = timeout(Duration::from_secs(3), session.next_payload(&cancel, false))
        .await
        .unwrap_or_else(|_| panic!("the closed relay must end the session"));
    assert!(
        matches!(end, Ok(None)),
        "a finished interval is not an error"
    );
    let request = server.await.unwrap_or_else(|error| panic!("{error}"));
    assert!(request.windows(10).any(|part| part == b"/playback?"));
    assert!(request.windows(15).any(|part| part == b"v3.9.0.20251120"));
    assert_eq!(&request[request.len() - 2..], [0x40, 0x0a], "field 8 = 10");
}

#[tokio::test]
async fn a_relay_closing_before_any_media_is_an_error_not_an_empty_clip() {
    let (listener, address) = listener().await;
    let server = tokio::spawn(async move {
        let (socket, _) = answer(listener, &[0x00]).await;
        drop(socket);
    });
    let url = format!("ysproto://{address}/playback?dev=fixture");
    let mut session = VtmSession::connect_playback(url, TIMEOUT)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let result = session.next_payload(&CancellationToken::new(), false).await;
    assert!(result.is_err());
    server.await.unwrap_or_else(|error| panic!("{error}"));
}

#[tokio::test]
async fn vendor_results_map_to_no_recording_and_offline() {
    // 5402 = 0x151a, 5405 = 0x151d as protobuf varints.
    for (path, result, playback) in [
        ("playback", [0x9a, 0x2a], true),
        ("live", [0x9d, 0x2a], false),
        ("playback", [0x9d, 0x2a], true),
    ] {
        let (listener, address) = listener().await;
        let server = tokio::spawn(async move { drop(answer(listener, &result).await) });
        let url = format!("ysproto://{address}/{path}?dev=fixture");
        let outcome = if playback {
            VtmSession::connect_playback(url, TIMEOUT).await
        } else {
            VtmSession::connect(url, TIMEOUT).await
        };
        match (result[0], outcome) {
            (0x9a, Err(BridgeError::SdNoRecording)) | (0x9d, Err(BridgeError::CameraOffline)) => {}
            (code, other) => panic!("result {code:#x}: {:?}", other.err()),
        }
        server.await.unwrap_or_else(|error| panic!("{error}"));
    }
}
