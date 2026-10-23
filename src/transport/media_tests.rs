//! Pre-signed media downloads: scheme/host rule, redirects and error detail,
//! against a loopback server reached through pinned host names.

use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    extract::State,
    http::{HeaderMap, StatusCode, header::LOCATION},
    response::{IntoResponse, Response},
    routing::get,
};
use reqwest::{Client, Url};

use super::media::{download_presigned, ezviz_media_host, media_client_builder, media_url_allowed};
use crate::error::BridgeError;

const SECRET_QUERY: &str = "Signature=presigned-secret";

struct Server {
    port: u16,
    hits: AtomicUsize,
}

async fn picture(State(server): State<Arc<Server>>, headers: HeaderMap) -> Response {
    server.hits.fetch_add(1, Ordering::SeqCst);
    let leaked = ["sessionid", "authorization", "featurecode", "cookie"]
        .iter()
        .any(|name| headers.contains_key(*name));
    if leaked {
        return StatusCode::BAD_REQUEST.into_response();
    }
    b"jpeg-bytes".to_vec().into_response()
}

fn redirect_to(server: &Server, host: &str) -> Response {
    let target = format!("http://{host}:{}/pic?{SECRET_QUERY}", server.port);
    (StatusCode::FOUND, [(LOCATION, target)]).into_response()
}

async fn serve() -> (Arc<Server>, Client) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let address: SocketAddr = listener
        .local_addr()
        .unwrap_or_else(|error| panic!("{error}"));
    let server = Arc::new(Server {
        port: address.port(),
        hits: AtomicUsize::new(0),
    });
    let app = Router::new()
        .route("/pic", get(picture))
        .route(
            "/to-ezviz",
            get(|State(s): State<Arc<Server>>| async move { redirect_to(&s, "cdn.ezvizlife.com") }),
        )
        .route(
            "/to-other",
            get(|State(s): State<Arc<Server>>| async move { redirect_to(&s, "media.example.net") }),
        )
        .route(
            "/loop",
            get(|| async { (StatusCode::FOUND, [(LOCATION, "/loop")]).into_response() }),
        )
        .with_state(Arc::clone(&server));
    tokio::spawn(async move { axum::serve(listener, app).await });
    let client = media_client_builder(Duration::from_secs(5))
        .resolve("img.ezvizlife.com", address)
        .resolve("cdn.ezvizlife.com", address)
        .resolve("media.example.net", address)
        .build()
        .unwrap_or_else(|error| panic!("{error}"));
    (server, client)
}

fn url(server: &Server, host: &str, path: &str) -> String {
    format!("http://{host}:{}{path}?{SECRET_QUERY}", server.port)
}

fn upstream_detail(result: Result<Vec<u8>, BridgeError>) -> String {
    match result {
        Err(BridgeError::Upstream(detail)) => detail,
        other => panic!("expected an upstream error, got {other:?}"),
    }
}

fn assert_no_credentials(detail: &str) {
    for secret in ["/pic", "Signature", "presigned-secret", "://", "?"] {
        assert!(!detail.contains(secret), "{detail}");
    }
}

#[tokio::test]
async fn plain_http_on_an_ezviz_host_is_downloaded_without_session_headers() {
    let (server, client) = serve().await;
    let body = download_presigned(&client, &url(&server, "img.ezvizlife.com", "/pic"), 1024).await;
    assert_eq!(body.ok(), Some(b"jpeg-bytes".to_vec()));
    let bounded = download_presigned(&client, &url(&server, "img.ezvizlife.com", "/pic"), 4).await;
    assert!(matches!(bounded, Err(BridgeError::Upstream(_))));
}

#[tokio::test]
async fn plain_http_elsewhere_is_rejected_naming_only_scheme_and_host() {
    let (server, client) = serve().await;
    let result =
        download_presigned(&client, &url(&server, "media.example.net", "/pic"), 1024).await;
    let detail = upstream_detail(result);
    assert!(
        detail.contains("scheme http, host media.example.net"),
        "{detail}"
    );
    assert_no_credentials(&detail);
    assert_eq!(server.hits.load(Ordering::SeqCst), 0, "nothing is fetched");
    let malformed = download_presigned(&client, "not a url", 1024).await;
    assert_eq!(upstream_detail(malformed), "media URL is malformed");
}

#[tokio::test]
async fn redirects_to_plain_http_follow_only_ezviz_hosts() {
    let (server, client) = serve().await;
    let get = |path: &'static str| {
        let target = url(&server, "img.ezvizlife.com", path);
        let client = client.clone();
        async move { download_presigned(&client, &target, 1024).await }
    };
    assert_eq!(get("/to-ezviz").await.ok(), Some(b"jpeg-bytes".to_vec()));
    let refused = get("/to-other");
    let detail = upstream_detail(refused.await);
    assert!(
        detail.contains("HTTP 302 (scheme http, host img.ezvizlife.com)"),
        "{detail}"
    );
    assert!(detail.contains("redirect refused to scheme http, host media.example.net"));
    assert_no_credentials(&detail);
    assert_eq!(
        server.hits.load(Ordering::SeqCst),
        1,
        "only the EZVIZ target"
    );
    assert!(upstream_detail(get("/loop").await).contains("HTTP 302"));
}

#[test]
fn ezviz_hosts_match_on_a_dot_boundary() {
    for host in [
        "ezvizlife.com",
        "img.ezvizlife.com",
        "A.B.EZVIZLIFE.COM.",
        "apiirus.ezvizru.com",
        "iusopen.ezviz.com",
    ] {
        assert!(ezviz_media_host(host), "{host}");
    }
    for host in [
        "evilezvizlife.com",
        ".ezvizlife.com",
        "ezvizlife.com.evil.example",
        "ezvizlife.co",
        "ys7.com",
        "hik-connect.com",
        "127.0.0.1",
        "",
    ] {
        assert!(!ezviz_media_host(host), "{host}");
    }
}

#[test]
fn https_stays_allowed_anywhere_and_other_schemes_never() {
    let allowed = |raw: &str| media_url_allowed(&Url::parse(raw).unwrap_or_else(|e| panic!("{e}")));
    assert!(allowed(
        "https://bucket.s3.eu-west-1.amazonaws.com/p.jpg?X-Amz-Signature=x"
    ));
    assert!(allowed("https://img.ezvizlife.com/p.jpg"));
    assert!(allowed("http://img.ezvizlife.com/p.jpg"));
    assert!(allowed("http://user@img.ezvizlife.com:8080/p.jpg"));
    assert!(!allowed("http://img.ezvizlife.com.evil.example/p.jpg"));
    assert!(!allowed("http://img.ezvizlife.com@evil.example/p.jpg"));
    assert!(!allowed("http://127.0.0.1/p.jpg"));
    assert!(!allowed("ftp://img.ezvizlife.com/p.jpg"));
    assert!(!allowed("file:///etc/passwd"));
}
