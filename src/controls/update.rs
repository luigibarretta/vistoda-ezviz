//! Verified control writes, PTZ steps and the account defence mode.
//!
//! Every write is explicit, serialized, preceded by an expected-value check
//! and followed by a bounded read-back; a disagreeing read-back triggers one
//! rollback and an `unconfirmed` error.

use serde_json::json;

use super::{
    AccountDefence, ControlError, ControlRequest, ControlService, ControlSource, Controls,
    DefenceMode, DefenceUpdate, PtzAction, PtzRequest, VendorWrite,
    cell::Cached,
    fields, parse_group_mode,
    service::{Confirmation, Refresh, confirm},
    write::{ControlKey, current, parse_key, parse_value, vendor_write},
};
use crate::config::CameraConfig;

impl ControlService {
    /// Applies one control change and returns the confirmed controls.
    pub async fn update(
        &self,
        alias: &str,
        request: &ControlRequest,
    ) -> Result<Controls, ControlError> {
        let config = self.camera(alias)?;
        let key = parse_key(&request.key)?;
        let desired = parse_value(key, &request.value)?;
        let expected = parse_value(key, &request.expected_value)?;
        let source = self.source()?;
        let source = source.as_ref();
        let refresh = if key == ControlKey::Sensitivity {
            Refresh::All
        } else {
            Refresh::Status
        };
        let _guard = self.writes.lock().await;
        let before = self.read(source, alias, config, refresh).await?;
        let previous = current(&before, key).ok_or(ControlError::Unsupported)?;
        if previous != expected {
            return Err(ControlError::Conflict(previous.to_json()));
        }
        if previous == desired {
            return Ok(before);
        }
        let undo = vendor_write(key, previous, &before)?;
        self.send(source, config, vendor_write(key, desired, &before)?)
            .await?;
        let read = move || self.read(source, alias, config, refresh);
        match confirm(read, |after: &Controls| {
            current(after, key) == Some(desired)
        })
        .await
        {
            Confirmation::Confirmed(after) => Ok(after),
            Confirmation::Mismatch => {
                tracing::warn!("control change was not confirmed; rolling back once");
                let _ignored = self.send(source, config, undo).await;
                self.invalidate().await;
                Err(ControlError::Unconfirmed)
            }
            Confirmation::Unknown => {
                self.invalidate().await;
                Err(ControlError::Unconfirmed)
            }
        }
    }

    async fn send(
        &self,
        source: &dyn ControlSource,
        config: &CameraConfig,
        write: VendorWrite,
    ) -> Result<(), ControlError> {
        if let Err(error) = source.write(config, write).await {
            self.invalidate().await;
            return Err(ControlError::upstream(&error));
        }
        Ok(())
    }

    /// One START+STOP step at the default speed; STOP is retried once so a
    /// transient failure does not leave the camera turning.
    pub async fn ptz(&self, alias: &str, request: PtzRequest) -> Result<(), ControlError> {
        let config = self.camera(alias)?;
        let source = self.source()?;
        let _guard = self.writes.lock().await;
        let snapshot = self.snapshot(source.as_ref(), false).await?;
        let supported = snapshot
            .get(&config.serial)
            .is_some_and(|data| fields::ptz(&data.device));
        if !supported {
            return Err(ControlError::Unsupported);
        }
        let direction = request.direction;
        let camera = config.clone();
        // A detached task outlives a cancelled request, and STOP follows every
        // attempted START: EZVIZ may forward a move whose answer was lost.
        let step = tokio::spawn(async move {
            let start = source.ptz(&camera, direction, PtzAction::Start).await;
            let mut stop = source.ptz(&camera, direction, PtzAction::Stop).await;
            if stop.is_err() {
                stop = source.ptz(&camera, direction, PtzAction::Stop).await;
            }
            start.and(stop)
        });
        step.await
            .map_err(|_| ControlError::Upstream)?
            .map_err(|error| ControlError::upstream(&error))
    }

    /// Cached account-level defence mode.
    pub async fn account_defence(&self) -> Result<AccountDefence, ControlError> {
        let source = self.source()?;
        let mode = self.defence_mode(source.as_ref(), false).await?;
        Ok(AccountDefence { mode })
    }

    async fn defence_mode(
        &self,
        source: &dyn ControlSource,
        force: bool,
    ) -> Result<Option<DefenceMode>, ControlError> {
        let fetch =
            move || async move { Ok(parse_group_mode(&source.group_defence_mode().await?)) };
        match self.defence.get(force, fetch).await {
            Cached::Value(mode) => Ok(mode),
            Cached::Failed => Err(ControlError::Upstream),
        }
    }

    /// Changes the account defence mode with the same verification rules.
    pub async fn set_account_defence(
        &self,
        update: DefenceUpdate,
    ) -> Result<AccountDefence, ControlError> {
        let source = self.source()?;
        let source = source.as_ref();
        let _guard = self.writes.lock().await;
        let previous = self.defence_mode(source, true).await?;
        if previous != update.expected_mode {
            return Err(ControlError::Conflict(json!(previous)));
        }
        if previous == Some(update.mode) {
            return Ok(AccountDefence { mode: previous });
        }
        self.send_mode(source, update.mode).await?;
        let read = move || self.defence_mode(source, true);
        match confirm(read, |mode| *mode == Some(update.mode)).await {
            Confirmation::Confirmed(mode) => Ok(AccountDefence { mode }),
            outcome => {
                if let (Confirmation::Mismatch, Some(mode)) = (outcome, previous) {
                    tracing::warn!("defence mode change was not confirmed; rolling back once");
                    let _ignored = self.send_mode(source, mode).await;
                }
                self.invalidate().await;
                Err(ControlError::Unconfirmed)
            }
        }
    }

    async fn send_mode(
        &self,
        source: &dyn ControlSource,
        mode: DefenceMode,
    ) -> Result<(), ControlError> {
        if let Err(error) = source.set_group_defence_mode(mode.code()).await {
            self.invalidate().await;
            return Err(ControlError::upstream(&error));
        }
        Ok(())
    }
}
