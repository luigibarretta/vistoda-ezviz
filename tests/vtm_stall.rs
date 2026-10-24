//! A VTM session must end when the camera stops sending media, even while the
//! relay keeps the socket busy with control packets.

use std::time::Duration;

use ezviz_vtm_bridge::{
    error::BridgeError,
    vtm::{
        CHANNEL_MESSAGE, CHANNEL_STREAM, KEEPALIVE_RESPONSE, STREAM_INFO_RESPONSE, VtmSession,
        decode_header, encode_packet,
    },
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::{Instant, sleep, timeout},
};
use tokio_util::sync::CancellationToken;

const SESSION_TIMEOUT: Duration = Duration::from_secs(1);

async fn handshake(listener: TcpListener) -> TcpStream {
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
    let body = [
        0x08, 0x00, 0x22, 0x07, b's', b'e', b's', b's', b'i', b'o', b'n',
    ];
    send(&mut socket, CHANNEL_MESSAGE, STREAM_INFO_RESPONSE, &body).await;
    socket
}

async fn send(socket: &mut TcpStream, channel: u8, code: u16, body: &[u8]) {
    let packet = encode_packet(body, channel, 0, code).unwrap_or_else(|error| panic!("{error}"));
    // The client may already have closed the socket; that ends the fixture.
    let _ignored = socket.write_all(&packet).await;
}

#[tokio::test]
async fn control_traffic_without_media_ends_the_session_as_camera_offline() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    let server = tokio::spawn(async move {
        let mut socket = handshake(listener).await;
        send(&mut socket, CHANNEL_STREAM, 0, b"\x00\x00\x01\xba-first").await;
        // Keepalive-style replies every 200 ms: each one used to restart the
        // per-read timeout, so a silent camera held the session forever.
        for _ in 0..40 {
            sleep(Duration::from_millis(200)).await;
            send(&mut socket, CHANNEL_MESSAGE, KEEPALIVE_RESPONSE, &[]).await;
        }
    });
    let url = format!("ysproto://{address}/live?dev=fixture&ssn=fixture");
    let mut session = VtmSession::connect(url, SESSION_TIMEOUT)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let cancel = CancellationToken::new();
    let first = session.next_payload(&cancel, false).await;
    assert!(matches!(first, Ok(Some(ref body)) if body.ends_with(b"-first")));
    let started = Instant::now();
    let stalled = timeout(Duration::from_secs(4), session.next_payload(&cancel, false))
        .await
        .unwrap_or_else(|_| panic!("a media stall must not hold the session open"));
    assert!(matches!(stalled, Err(BridgeError::CameraOffline)));
    assert!(started.elapsed() < Duration::from_millis(1_800));
    drop(session);
    server.abort();
}

#[tokio::test]
async fn steady_media_is_never_mistaken_for_a_stall() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    let server = tokio::spawn(async move {
        let mut socket = handshake(listener).await;
        for _ in 0..10 {
            send(&mut socket, CHANNEL_STREAM, 0, b"\x00\x00\x01\xba-frame").await;
            sleep(Duration::from_millis(300)).await;
        }
    });
    let url = format!("ysproto://{address}/live?dev=fixture&ssn=fixture");
    let mut session = VtmSession::connect(url, SESSION_TIMEOUT)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let cancel = CancellationToken::new();
    for _ in 0..10 {
        let payload = session.next_payload(&cancel, false).await;
        assert!(
            matches!(payload, Ok(Some(_))),
            "media spaced under the timeout"
        );
    }
    server.await.unwrap_or_else(|error| panic!("{error}"));
}
