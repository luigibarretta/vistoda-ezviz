use chrono::Utc;
use reqwest::Method;
use serde_json::Value;

use crate::{
    config::CameraConfig,
    error::BridgeError,
    transport::{
        client::{EzvizTransport, success_code},
        http::{find_resource, jwt_sign, required_string, value_u16},
    },
    vtm::{VtmUrlRequest, build_vtm_url},
};

impl EzvizTransport {
    pub(super) async fn cloud_stream_url(
        &self,
        camera: &CameraConfig,
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
        Ok(build_vtm_url(&VtmUrlRequest {
            host,
            port,
            serial: &camera.serial,
            channel: camera.channel,
            substream: camera.substream,
            biz_url,
            token: &vtdu,
            timestamp_ms: Utc::now().timestamp_millis(),
        }))
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
        let token = self.token.read().await.clone();
        let sign = jwt_sign(&token.session_id)?;
        let url = format!("{}/vtdutoken2", token.auth_address()?);
        let response = self
            .request(Method::GET, &url, &token)
            .query(&[("ssid", &token.session_id), ("sign", &sign)])
            .send()
            .await?;
        let status = response.status();
        let value: Value = response.json().await?;
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
