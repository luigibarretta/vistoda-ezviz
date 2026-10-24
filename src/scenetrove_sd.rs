//! Imports one day of microSD clips into a `SceneTrove` device folder.
//!
//! The day's record index is read from the bridge; every interval not yet in
//! the destination's ledger is pulled as a microSD recording and named after
//! its recording time (ADR-0024). Each pull wakes the camera, so a run makes
//! at most `max_clips` attempts; the next run resumes where it stopped, and a
//! clip that failed is retried with a new key a bounded number of times.

use std::collections::{BTreeMap, BTreeSet};

use reqwest::Client;
use serde::{Deserialize, Serialize};

use super::{
    PullRequest, PullResult, encode_segment, pull, transfer::bounded_json, validated_base_url,
};
use crate::{
    error::BridgeError,
    storage::{atomic_write_json, ensure_private_regular},
};

const LEDGER: &str = ".sd-ledger.json";
/// Prefix of `pull`'s error for a recording the bridge marked failed.
const FAILED: &str = "bridge recording failed: ";
/// Prefix of `pull`'s error when the bridge refuses the request (key reuse).
const KEY_REFUSED: &str = "bridge returned HTTP 400";
/// `pull`'s error when the bridge is still working on a recording; the same
/// key picks it up on the next run.
const STILL_RUNNING: &str = "recording did not finish before deadline";
/// Bridge recordings tried per clip before it is given up.
const MAX_ATTEMPTS: u32 = 3;

/// Clips imported or given up, and failed attempts of the others.
#[derive(Default, Deserialize, Serialize)]
struct Ledger {
    done: BTreeSet<String>,
    attempts: BTreeMap<String, u32>,
}

impl Ledger {
    fn finish(&mut self, clip: &str) {
        self.attempts.remove(clip);
        self.done.insert(clip.to_owned());
    }
}

#[derive(Deserialize)]
struct Record {
    start: i64,
    end: i64,
}

#[derive(Deserialize)]
struct Records {
    records: Vec<Record>,
}

/// Limits of one `--sd-day` run.
pub struct SdDay {
    /// Camera-local `YYYY-MM-DD`, as accepted by `GET …/sd-records`.
    pub day: String,
    pub max_clips: usize,
    /// Longest interval requested; longer clips are imported from their start.
    pub max_seconds: i64,
}

/// Pulls the day's not-yet-imported clips; `template` supplies the bridge,
/// camera, credentials and destination.
pub async fn pull_day(template: &PullRequest, day: &SdDay) -> Result<Vec<PullResult>, BridgeError> {
    let base = validated_base_url(&template.base_url)?;
    let token = ensure_private_regular(&template.token_file, 32)?;
    let url = base
        .join(&format!(
            "v1/cameras/{}/sd-records",
            encode_segment(&template.camera)
        ))
        .map_err(|_| BridgeError::Configuration("records URL is invalid".into()))?;
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(template.timeout_seconds))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    let response = client
        .get(url)
        .query(&[("date", &day.day)])
        .bearer_auth(&token)
        .send()
        .await?;
    let mut records = bounded_json::<Records>(response).await?.records;
    records.sort_by_key(|record| record.start);
    tokio::fs::create_dir_all(&template.destination).await?;
    let ledger_path = template.destination.join(LEDGER);
    let mut ledger: Ledger = match tokio::fs::read(&ledger_path).await {
        Ok(bytes) => serde_json::from_slice(&bytes)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ledger::default(),
        Err(error) => return Err(error.into()),
    };
    let mut pulled = Vec::new();
    let mut attempts_left = day.max_clips;
    for record in records.iter().filter(|record| record.end >= record.start) {
        if attempts_left == 0 {
            break;
        }
        // The clip's identity is the listed interval, whatever bound is used.
        let clip = format!("sd-{}-{}-{}", template.camera, record.start, record.end);
        if ledger.done.contains(&clip) {
            continue;
        }
        // Imported but not recorded here (a crash after the bridge ACK).
        if already_imported(&template.destination, record.start)? {
            ledger.finish(&clip);
            atomic_write_json(&ledger_path, &ledger)?;
            continue;
        }
        attempts_left -= 1;
        let attempt = ledger.attempts.get(&clip).copied().unwrap_or(0);
        let end = record
            .end
            .min(record.start.saturating_add(day.max_seconds))
            .max(record.start + 1);
        let request = PullRequest {
            sd_interval: Some((record.start, end)),
            // A failed or acknowledged bridge recording keeps its key, and a
            // different interval may not reuse it: each attempt gets its own.
            idempotency_key: format!("{clip}-e{end}-a{attempt}"),
            ..template.clone()
        };
        match pull(request).await {
            Ok(result) => {
                pulled.push(result);
                ledger.finish(&clip);
            }
            // Overwritten on the card: never retry.
            Err(BridgeError::Upstream(detail)) if detail.ends_with("sd_no_recording") => {
                ledger.finish(&clip);
            }
            // The camera did not wake or the relay failed: retry on a later
            // run with a new key, at most `MAX_ATTEMPTS` times.
            Err(BridgeError::Upstream(detail))
                if detail.starts_with(FAILED) || detail.starts_with(KEY_REFUSED) =>
            {
                if attempt + 1 >= MAX_ATTEMPTS {
                    ledger.finish(&clip);
                } else {
                    ledger.attempts.insert(clip, attempt + 1);
                }
            }
            // Slower than this run waits (playback runs about in real time):
            // move on; the next run resumes the same recording by its key.
            Err(BridgeError::Upstream(detail)) if detail == STILL_RUNNING => {}
            // The bridge itself is unreachable or misbehaving: stop the run.
            Err(error) => {
                atomic_write_json(&ledger_path, &ledger)?;
                return Err(error);
            }
        }
        atomic_write_json(&ledger_path, &ledger)?;
    }
    Ok(pulled)
}

/// A file of this clip's recording time is already in the destination.
fn already_imported(destination: &std::path::Path, start: i64) -> Result<bool, BridgeError> {
    let Some(stamp) = chrono::DateTime::from_timestamp(start, 0) else {
        return Ok(false);
    };
    let prefix = format!("hiv{}-", stamp.format("%Y%m%dT%H%M%SZ"));
    for entry in std::fs::read_dir(destination)? {
        if entry?.file_name().to_string_lossy().starts_with(&prefix) {
            return Ok(true);
        }
    }
    Ok(false)
}
