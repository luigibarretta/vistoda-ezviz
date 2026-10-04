//! Summary-driven alarm poller with silent startup priming and backoff.

use std::{
    collections::{BTreeMap, VecDeque},
    sync::Arc,
    time::Duration,
};

use tokio_util::sync::CancellationToken;

use super::{
    AlarmFeed, AlarmRecord, AlarmSource, PictureError, decode_with_keys,
    fetch::{LIVE_PAGES, PRIME_PAGES, fetch_unknown},
    model::{ParsedMessage, parse_summary},
    pictures::MAX_PICTURE_BYTES,
};
use crate::error::BridgeError;

const MAX_BACKOFF: Duration = Duration::from_secs(300);
const PICTURE_TIMEOUT: Duration = Duration::from_secs(15);
/// Pictures downloaded per camera per cycle (newest alarms first).
const PICTURES_PER_CYCLE: usize = 10;
/// Recently seen IDs per serial, including channels that are not configured.
const SEEN_PER_SERIAL: usize = 512;

#[derive(Default)]
struct SerialState {
    baseline: Option<(Option<String>, u64)>,
    offset: Option<i32>,
    seen: VecDeque<String>,
}

pub async fn run(
    feed: Arc<AlarmFeed>,
    source: Arc<dyn AlarmSource>,
    interval: Duration,
    shutdown: CancellationToken,
) {
    let mut serials: BTreeMap<String, SerialState> = BTreeMap::new();
    let mut delay = Duration::ZERO;
    loop {
        tokio::select! {
            () = shutdown.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
        let failed = cycle(&feed, source.as_ref(), &mut serials).await;
        delay = if failed {
            delay.max(interval).saturating_mul(2).min(MAX_BACKOFF)
        } else {
            interval
        };
    }
}

/// Runs one poll cycle and reports whether any upstream call failed.
async fn cycle(
    feed: &AlarmFeed,
    source: &dyn AlarmSource,
    serials: &mut BTreeMap<String, SerialState>,
) -> bool {
    let summary = match source.alarm_summary().await {
        Ok(value) => parse_summary(&value),
        Err(error) => {
            report(feed, None, &error).await;
            return true;
        }
    };
    let mut groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (alias, slot) in &feed.cameras {
        feed.metrics.increment("alarm_polls_total", alias).await;
        groups
            .entry(slot.config.serial.clone())
            .or_default()
            .push(alias.clone());
    }
    let mut failed = false;
    for (serial, aliases) in groups {
        let state = serials.entry(serial.clone()).or_default();
        let device = summary.iter().find(|device| device.serial == serial);
        if let Some(offset) = device.and_then(|device| device.utc_offset) {
            state.offset = Some(offset);
        }
        let current = device.map_or((None, 0), |device| (device.top_id.clone(), device.total));
        if state.baseline.as_ref() == Some(&current) {
            continue;
        }
        let live = state.baseline.is_some();
        if let Err(error) = sync_serial(feed, source, &serial, &aliases, state, live).await {
            report(feed, Some(&aliases), &error).await;
            failed = true;
            continue;
        }
        state.baseline = Some(current);
    }
    failed
}

async fn sync_serial(
    feed: &AlarmFeed,
    source: &dyn AlarmSource,
    serial: &str,
    aliases: &[String],
    state: &mut SerialState,
    live: bool,
) -> Result<(), BridgeError> {
    let (known, newest) = support::known_ids(feed, aliases, state).await;
    let pages = if live { LIVE_PAGES } else { PRIME_PAGES };
    let is_known = |id: &str| known.contains(id);
    let result = fetch_unknown(source, serial, state.offset, newest, &is_known, pages).await?;
    for alias in aliases {
        let Some(slot) = feed.cameras.get(alias) else {
            continue;
        };
        if live && result.truncated {
            feed.metrics
                .increment("alarm_backlog_truncated_total", alias)
                .await;
        }
        let channel = slot.config.channel;
        let mine: Vec<&ParsedMessage> = result
            .messages
            .iter()
            .filter(|message| message.channel.filter(|value| *value > 0).unwrap_or(1) == channel)
            .collect();
        if mine.is_empty() {
            continue;
        }
        let batch = with_pictures(feed, source, alias, &mine).await;
        let outcome = feed.publish(alias, batch, live).await?;
        support::record_outcome(feed, alias, &outcome, live).await;
    }
    // Remember IDs only after every alias committed them, so a failed
    // publish is retried instead of being treated as already known.
    for message in &result.messages {
        state.seen.push_back(message.record.id.clone());
    }
    while state.seen.len() > SEEN_PER_SERIAL {
        state.seen.pop_front();
    }
    Ok(())
}

/// Downloads and decodes pictures for the newest alarms within the budget.
async fn with_pictures(
    feed: &AlarmFeed,
    source: &dyn AlarmSource,
    alias: &str,
    messages: &[&ParsedMessage],
) -> Vec<(AlarmRecord, Option<Vec<u8>>)> {
    let mut device_keys: Option<Vec<String>> = None;
    let first_with_picture = messages.len().saturating_sub(PICTURES_PER_CYCLE);
    let mut batch = Vec::with_capacity(messages.len());
    for (index, message) in messages.iter().enumerate() {
        let mut jpeg = None;
        if let (true, Some(picture)) = (index >= first_with_picture, &message.picture) {
            if picture.crypt == 1 && device_keys.is_none() {
                let config = feed.cameras.get(alias).map(|slot| &slot.config);
                let keys = match config {
                    Some(config) => source.picture_keys(config).await.unwrap_or_default(),
                    None => Vec::new(),
                };
                device_keys = Some(keys);
            }
            let download = tokio::time::timeout(
                PICTURE_TIMEOUT,
                source.alarm_picture(&picture.url, MAX_PICTURE_BYTES),
            )
            .await;
            let decoded = match download {
                Ok(Ok(raw)) => decode_with_keys(
                    &raw,
                    picture.crypt,
                    picture.checksum.as_deref(),
                    device_keys.as_deref().unwrap_or_default(),
                ),
                _ => Err(PictureError::Invalid),
            };
            match decoded {
                Ok(data) => jpeg = Some(data),
                Err(kind) => {
                    if kind == PictureError::Unsupported {
                        feed.metrics
                            .increment("alarm_pictures_unsupported_total", alias)
                            .await;
                    }
                    feed.metrics
                        .increment("alarm_pictures_failed_total", alias)
                        .await;
                }
            }
        }
        batch.push((message.record.clone(), jpeg));
    }
    batch
}

async fn report(feed: &AlarmFeed, aliases: Option<&[String]>, error: &BridgeError) {
    tracing::warn!(
        error_type = error.diagnostic_code(),
        detail = error.diagnostic_detail(),
        "alarm poll failed"
    );
    let all: Vec<String> = feed.cameras.keys().cloned().collect();
    for alias in aliases.unwrap_or(&all) {
        feed.metrics
            .increment("alarm_poll_errors_total", alias)
            .await;
    }
}

mod support;
