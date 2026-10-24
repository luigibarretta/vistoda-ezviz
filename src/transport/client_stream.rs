use std::sync::Arc;

use chrono::Utc;
use reqwest::Method;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::{
    config::CameraConfig,
    error::BridgeError,
    transport::{
        ChunkConsumer,
        authed::{http_error, read_json},
        client::{EzvizTransport, success_code},
        http::{find_resource, jwt_sign, required_string, value_u16},
        timeout_duration,
        video::{self, RtpVideo},
        video_pipeline,
    },
    vtm::{VtmSession, VtmUrlRequest, build_vtm_playback_url, build_vtm_url},
};

impl EzvizTransport {
    /// Live media of a camera without video encryption.
    pub(super) async fn stream_clear(
        &self,
        camera: &CameraConfig,
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        let url = self.cloud_stream_url(camera).await?;
        let session = VtmSession::connect(url, timeout_duration(self.timeout_seconds)).await?;
        Self::copy_clear(session, cancel, output).await
    }

    /// Media of an open clear session, live or microSD playback.
    pub(super) async fn copy_clear(
        mut session: VtmSession,
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        // The relay forwards whatever the camera produces: MPEG-PS on some
        // models, RTP on others even with encryption off. The first payload
        // decides; MPEG-PS keeps the untouched pass-through.
        let first = loop {
            match session.next_payload(&cancel, false).await? {
                None => return Ok(()),
                Some(payload) if payload.is_empty() => {}
                Some(payload) => break payload,
            }
        };
        if video::is_rtp(&first) {
            tracing::info!("camera sends RTP without encryption; muxing it in-process");
            return video_pipeline::copy_as_mpeg_ts(
                session,
                RtpVideo::clear(),
                Some(first),
                false,
                cancel,
                output,
            )
            .await;
        }
        if !output.consume(first) {
            return Ok(());
        }
        session
            .copy_payloads(&cancel, |chunk| output.consume(chunk))
            .await
    }

    pub(super) async fn cloud_stream_url(
        &self,
        camera: &CameraConfig,
    ) -> Result<String, BridgeError> {
        self.cloud_url(camera, None).await
    }

    /// `begin`/`end` are camera-local `yyyyMMddTHHmmssZ`.
    pub(super) async fn cloud_playback_url(
        &self,
        camera: &CameraConfig,
        begin: &str,
        end: &str,
    ) -> Result<String, BridgeError> {
        self.cloud_url(camera, Some((begin, end))).await
    }

    async fn cloud_url(
        &self,
        camera: &CameraConfig,
        interval: Option<(&str, &str)>,
    ) -> Result<String, BridgeError> {
        // A VTDU token is one-shot and belongs to the immediately following
        // allocation, so concurrent camera allocations must be serialized.
        let _allocation = self.allocation_lock.lock().await;
        let page = self.resource_page(&camera.serial, camera.channel).await?;
        let resource = find_resource(&page, &camera.serial, camera.channel)?;
        let resource_id = required_string(resource, "resourceId")?;
        let biz_url = resource
            .get("streamBizUrl")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let vtdu = self.vtdu_token().await?;
        let server = self
            .api_json(
                Method::GET,
                &format!("/v3/streaming/vtm/{}/{}", camera.serial, camera.channel),
                &[],
            )
            .await?;
        let server = server
            .get("streamServerConfig")
            .or_else(|| page.get("VTM").and_then(|value| value.get(&resource_id)))
            .ok_or_else(|| BridgeError::Upstream("VTM server metadata is missing".into()))?;
        let host = ["externalIp", "domain", "internalIp"]
            .iter()
            .find_map(|key| server.get(*key).and_then(Value::as_str))
            .ok_or_else(|| BridgeError::Upstream("VTM server host is missing".into()))?;
        let port = value_u16(server.get("port"))?;
        let request = VtmUrlRequest {
            host,
            port,
            serial: &camera.serial,
            channel: camera.channel,
            substream: camera.substream,
            biz_url,
            token: &vtdu,
            timestamp_ms: Utc::now().timestamp_millis(),
        };
        Ok(match interval {
            None => build_vtm_url(&request),
            Some((begin, end)) => build_vtm_playback_url(&request, begin, end),
        })
    }

    async fn resource_page(&self, serial: &str, channel: u16) -> Result<Value, BridgeError> {
        const PAGE_SIZE: usize = 50;
        const MAX_PAGES: usize = 40;
        let mut offset = 0_usize;
        for _ in 0..MAX_PAGES {
            let page = self
                .api_json(
                    Method::GET,
                    "/v3/userdevices/v1/resources/pagelist",
                    &[
                        ("groupId", "-1".into()),
                        ("limit", PAGE_SIZE.to_string()),
                        ("offset", offset.to_string()),
                        ("filter", "VTM".into()),
                    ],
                )
                .await?;
            if find_resource(&page, serial, channel).is_ok() {
                return Ok(page);
            }
            let resources = page
                .get("resourceInfos")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            let has_next = page
                .get("page")
                .and_then(|value| value.get("hasNext"))
                .and_then(Value::as_bool)
                .unwrap_or(resources == PAGE_SIZE);
            if resources == 0 || !has_next {
                break;
            }
            offset = offset.saturating_add(resources);
        }
        Err(BridgeError::Upstream(
            "camera channel was not found in the complete VTM resource list".into(),
        ))
    }

    async fn vtdu_token(&self) -> Result<String, BridgeError> {
        let token = self.token().await;
        let sign = jwt_sign(&token.session_id)?;
        let url = format!("{}/vtdutoken2", token.auth_address()?);
        // The query carries the session id: errors must drop the URL.
        let response = self
            .request(Method::GET, &url, &token)
            .query(&[("ssid", &token.session_id), ("sign", &sign)])
            .send()
            .await
            .map_err(http_error)?;
        let status = response.status();
        let value = read_json(response, status.is_success()).await?;
        if !status.is_success() || !success_code(value.get("retcode")) {
            return Err(BridgeError::Upstream("VTDU token request failed".into()));
        }
        value
            .get("tokens")
            .and_then(Value::as_array)
            .and_then(|tokens| tokens.first())
            .and_then(Value::as_str)
            .map(str::to_owned)
            .ok_or_else(|| BridgeError::Upstream("VTDU response omitted its token".into()))
    }
}
