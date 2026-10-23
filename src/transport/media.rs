//! Downloads of pre-signed EZVIZ media URLs (capture snapshots, alarm pictures).
//!
//! Security reasoning (see also `docs/THREAT_MODEL.md` and ADR-0022):
//!
//! - No EZVIZ session, bearer or feature-code header is ever attached; the URL
//!   alone authorizes the download, exactly as EZVIZ handed it out.
//! - HTTPS URLs are accepted on any host. Plain HTTP is accepted only on
//!   EZVIZ-owned domains, because EZVIZ can return `http://` capture URLs
//!   (pyEzvizApi accepts both schemes for `picUrl`). A man-in-the-middle on
//!   that path could read or replace the picture, but obtains no credential:
//!   the pre-signed URL authorizes only that picture, and when the camera
//!   uses its verification code the body is additionally AES-encrypted
//!   (`hikencodepicture`, decrypted locally with the stored key).
//! - Redirects follow the same rule, at most three times.
//! - Bodies are read with a size bound; errors name only scheme and host,
//!   never the path or query, which carry the pre-signed credentials.

use reqwest::{Client, Response, StatusCode, Url, header::LOCATION, redirect};

use crate::{
    error::BridgeError,
    transport::authed::{http_error, read_limited},
};

/// EZVIZ-owned registrable domains allowed to serve media over plain HTTP.
/// - `ezvizlife.com`: every global API, auth and CAS host (`token.rs`,
///   `enroll_api.rs`, pyEzvizApi defaults such as `apiieu.ezvizlife.com`).
/// - `ezvizru.com`: the Russian region's rewrite of `ezvizlife`
///   (pyEzvizApi `hcnetsdk.py`, `enroll_api.rs`).
/// - `ezviz.com`: EZVIZ's own corporate and open-platform domain
///   (`iusopen.ezviz.com`, docs/RESEARCH.md).
const PLAIN_HTTP_MEDIA_DOMAINS: [&str; 3] = ["ezvizlife.com", "ezvizru.com", "ezviz.com"];
const MAX_REDIRECTS: usize = 3;

/// True for an EZVIZ-owned host: an exact domain or a subdomain of one.
pub(super) fn ezviz_media_host(host: &str) -> bool {
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    PLAIN_HTTP_MEDIA_DOMAINS.iter().any(|domain| {
        host == *domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.len() > 1 && prefix.ends_with('.'))
    })
}

/// HTTPS anywhere; plain HTTP only on EZVIZ-owned hosts; nothing else.
pub(super) fn media_url_allowed(url: &Url) -> bool {
    match url.scheme() {
        "https" => url.host_str().is_some(),
        "http" => url.host_str().is_some_and(ezviz_media_host),
        _ => false,
    }
}

/// Scheme and host only, without `://` so log redaction keeps it readable.
fn origin(url: &Url) -> String {
    format!(
        "scheme {}, host {}",
        url.scheme(),
        url.host_str().unwrap_or("none")
    )
}

/// Client for pre-signed media URLs: no session headers are ever attached,
/// and redirects follow [`media_url_allowed`], at most three times.
pub(super) fn media_client_builder(timeout: std::time::Duration) -> reqwest::ClientBuilder {
    Client::builder()
        .timeout(timeout)
        .redirect(redirect::Policy::custom(|attempt| {
            if attempt.previous().len() < MAX_REDIRECTS && media_url_allowed(attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
}

pub(super) fn media_client(timeout: std::time::Duration) -> Result<Client, BridgeError> {
    Ok(media_client_builder(timeout).build()?)
}

/// Downloads one pre-signed media URL into at most `max` bytes.
pub(super) async fn download_presigned(
    client: &Client,
    url: &str,
    max: usize,
) -> Result<Vec<u8>, BridgeError> {
    let parsed =
        Url::parse(url).map_err(|_| BridgeError::Upstream("media URL is malformed".into()))?;
    if !media_url_allowed(&parsed) {
        return Err(BridgeError::Upstream(format!(
            "media URL is not HTTPS or an EZVIZ host ({})",
            origin(&parsed)
        )));
    }
    let response = client.get(parsed).send().await.map_err(http_error)?;
    if !response.status().is_success() {
        return Err(BridgeError::Upstream(failed_download(&response)));
    }
    read_limited(response, max).await
}

/// Status and origin of a failed download; a refused redirect names its target.
fn failed_download(response: &Response) -> String {
    let status = response.status();
    let detail = format!(
        "media download returned HTTP {} ({})",
        status.as_u16(),
        origin(response.url())
    );
    if !status.is_redirection() || status == StatusCode::NOT_MODIFIED {
        return detail;
    }
    let target = response
        .headers()
        .get(LOCATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| response.url().join(value).ok());
    target.map_or_else(
        || format!("{detail}, redirect without a usable location"),
        |target| format!("{detail}, redirect refused to {}", origin(&target)),
    )
}
