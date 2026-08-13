use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use ezviz_vtm_bridge::vtm::{
    CHANNEL_MESSAGE, CHANNEL_STREAM, STREAM_INFO_RESPONSE, VtmSession, decode_header, encode_packet,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn native_session_sends_golden_request_and_extracts_mpeg_ps() {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let address = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener
            .accept()
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let mut header = [0_u8; 8];
        socket
            .read_exact(&mut header)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let (channel, length, sequence, code) =
            decode_header(&header).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!((channel, sequence, code), (CHANNEL_MESSAGE, 0, 0x13b));
        let mut request = vec![0; length];
        socket
            .read_exact(&mut request)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(request.windows(10).any(|part| part == b"ysproto://"));
        let response_body = [
            0x08, 0x00, 0x22, 0x07, b's', b'e', b's', b's', b'i', b'o', b'n',
        ];
        let response = encode_packet(&response_body, CHANNEL_MESSAGE, 0, STREAM_INFO_RESPONSE)
            .unwrap_or_else(|error| panic!("{error}"));
        socket
            .write_all(&response)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        let media = encode_packet(b"\x00\x00\x01\xba-live", CHANNEL_STREAM, 1, 0)
            .unwrap_or_else(|error| panic!("{error}"));
        socket
            .write_all(&media)
            .await
            .unwrap_or_else(|error| panic!("{error}"));
    });
    let url = format!("ysproto://{address}/live?dev=fixture&ssn=fixture");
    let mut session = VtmSession::connect(url, Duration::from_secs(2))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let chunks = Arc::new(Mutex::new(Vec::new()));
    let target = Arc::clone(&chunks);
    session
        .copy_payloads(&CancellationToken::new(), move |chunk| {
            target
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(chunk);
            false
        })
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        chunks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_slice(),
        &[b"\x00\x00\x01\xba-live".to_vec()]
    );
    server.await.unwrap_or_else(|error| panic!("{error}"));
}
