use std::{convert::Infallible, sync::Arc, time::Duration};

use async_stream::stream;
use axum::{
    body::Body,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
};
use bytes::Bytes;
use tokio::{sync::mpsc, time::timeout};

use crate::error::BridgeError;

use super::{Runtime, no_store, response};

pub(super) async fn raw_stream(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
) -> Result<Response, BridgeError> {
    let hub = Arc::clone(
        runtime
            .raw
            .get(&camera)
            .ok_or(BridgeError::CameraNotFound)?,
    );
    let mut subscription = hub.subscribe().await?;
    runtime
        .metrics
        .increment("stream_requests_total", &camera)
        .await;
    let first = match first_chunk(
        &mut subscription.receiver,
        Duration::from_secs(runtime.config.upstream_timeout_seconds),
    )
    .await
    {
        Ok(chunk) => chunk,
        Err(error) => {
            hub.unsubscribe(subscription.id);
            return Err(error);
        }
    };
    let body = Body::from_stream(stream! {
        yield Ok::<_, Infallible>(first);
        while let Some(chunk) = subscription.receiver.recv().await {
            yield Ok::<_, Infallible>(chunk);
        }
        hub.unsubscribe(subscription.id);
    });
    let mut result = response(StatusCode::OK, "video/mpeg", body);
    no_store(result.headers_mut());
    Ok(result)
}

pub(super) async fn ts_stream(
    State(runtime): State<Arc<Runtime>>,
    Path(camera): Path<String>,
) -> Result<Response, BridgeError> {
    let hub = Arc::clone(runtime.ts.get(&camera).ok_or(BridgeError::CameraNotFound)?);
    let mut subscription = hub.subscribe().await?;
    runtime
        .metrics
        .increment("stream_requests_total", &camera)
        .await;
    let first = match first_chunk(
        &mut subscription.receiver,
        Duration::from_secs(runtime.config.upstream_timeout_seconds),
    )
    .await
    {
        Ok(chunk) => chunk,
        Err(error) => {
            hub.unsubscribe(subscription.id);
            return Err(error);
        }
    };
    let body = Body::from_stream(stream! {
        yield Ok::<_, Infallible>(first);
        while let Some(chunk) = subscription.receiver.recv().await {
            yield Ok::<_, Infallible>(chunk);
        }
        hub.unsubscribe(subscription.id);
    });
    let mut result = response(StatusCode::OK, "video/mp2t", body);
    no_store(result.headers_mut());
    Ok(result)
}

async fn first_chunk(
    receiver: &mut mpsc::Receiver<Bytes>,
    startup_timeout: Duration,
) -> Result<Bytes, BridgeError> {
    match timeout(startup_timeout, receiver.recv()).await {
        Ok(Some(chunk)) => Ok(chunk),
        Ok(None) | Err(_) => Err(BridgeError::UpstreamUnavailable),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn startup_fails_closed_when_upstream_yields_no_media() {
        let (sender, mut receiver) = mpsc::channel(1);
        drop(sender);
        assert!(matches!(
            first_chunk(&mut receiver, Duration::from_millis(10)).await,
            Err(BridgeError::UpstreamUnavailable)
        ));
    }

    #[tokio::test]
    async fn startup_forwards_the_first_media_chunk() {
        let (sender, mut receiver) = mpsc::channel(1);
        sender
            .send(Bytes::from_static(b"media"))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            first_chunk(&mut receiver, Duration::from_millis(10))
                .await
                .unwrap_or_else(|error| panic!("{error}")),
            Bytes::from_static(b"media")
        );
    }
}
