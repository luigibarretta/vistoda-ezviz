//! Bounded, persisted per-camera alarm history with process-local sequences.

use std::{
    collections::{BTreeSet, HashSet, VecDeque},
    path::PathBuf,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{model::AlarmRecord, pictures::PictureDir};
use crate::{config::CameraConfig, error::BridgeError, storage::atomic_write_json};

#[path = "store_read.rs"]
mod read;

/// Alarms (and therefore pictures) kept per camera unless configured.
pub const DEFAULT_HISTORY_LIMIT: usize = 200;
const SCHEMA_VERSION: u8 = 1;

#[derive(Deserialize, Serialize)]
struct HistoryFile {
    schema_version: u8,
    binding: String,
    events: Vec<AlarmRecord>,
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub sequence: u64,
    pub record: AlarmRecord,
}

/// Outcome counters for one inserted batch.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct InsertOutcome {
    pub inserted: usize,
    pub pictures_stored: usize,
    pub pictures_failed: usize,
}

pub struct CameraHistory {
    file: PathBuf,
    binding: String,
    pub(super) pictures: PictureDir,
    events: VecDeque<Entry>,
    /// Ids of `events`, so duplicate checks stay cheap at the largest limit.
    index: HashSet<String>,
    head: u64,
    floor: u64,
    picture_budget: u64,
    limit: usize,
}

/// Opaque binding so a re-pointed alias never inherits another camera's alarms.
fn binding(camera: &CameraConfig) -> String {
    hex::encode(Sha256::digest(
        format!("{}:{}", camera.serial, camera.channel).as_bytes(),
    ))
}

impl CameraHistory {
    /// Loads persisted history; corrupt or foreign state starts empty.
    pub fn load(
        directory: &std::path::Path,
        camera: &CameraConfig,
        picture_budget: u64,
        limit: usize,
    ) -> Self {
        let file = directory.join("history.json");
        let pictures = PictureDir::new(directory.join("pictures"));
        let binding = binding(camera);
        let stored = std::fs::read(&file)
            .ok()
            .and_then(|raw| serde_json::from_slice::<HistoryFile>(&raw).ok())
            .filter(|value| value.schema_version == SCHEMA_VERSION && value.binding == binding);
        let mut history = Self {
            file,
            binding,
            pictures,
            events: VecDeque::new(),
            index: HashSet::new(),
            head: 0,
            floor: 0,
            picture_budget,
            limit,
        };
        let records = stored.map(|value| value.events).unwrap_or_default();
        let skip = records.len().saturating_sub(limit);
        for mut record in records.into_iter().skip(skip) {
            if !super::model::is_token(&record.id) || history.knows(&record.id) {
                continue;
            }
            record.title = super::model::sanitize_title(&record.title);
            if record.has_picture && !history.pictures.exists(&record.id) {
                record.has_picture = false;
                record.picture_bytes = 0;
            }
            history.push(record);
        }
        history.floor = history.head;
        let keep = history.ids();
        history.pictures.remove_orphans(&keep);
        let dropped_pictures = history.enforce_picture_budget();
        // A lowered limit or budget: what was trimmed must leave the file (and backups).
        if skip > 0 || dropped_pictures {
            if let Err(error) = history.persist() {
                tracing::warn!(
                    error_type = error.diagnostic_code(),
                    "alarm history rewrite failed"
                );
            }
        }
        history
    }

    fn ids(&self) -> BTreeSet<String> {
        self.events
            .iter()
            .filter(|entry| entry.record.has_picture)
            .map(|entry| entry.record.id.clone())
            .collect()
    }

    fn push(&mut self, record: AlarmRecord) {
        self.head += 1;
        self.index.insert(record.id.clone());
        self.events.push_back(Entry {
            sequence: self.head,
            record,
        });
        while self.events.len() > self.limit {
            if let Some(evicted) = self.events.pop_front() {
                self.index.remove(&evicted.record.id);
                self.pictures.remove(&evicted.record.id);
            }
        }
    }

    #[must_use]
    pub fn knows(&self, id: &str) -> bool {
        self.index.contains(id)
    }

    /// A full history ignores alarms older than its oldest one: they were
    /// evicted (or would be at once) and must not come back as new.
    fn predates_full_history(&self, record: &AlarmRecord) -> bool {
        self.events.len() >= self.limit
            && self.events.front().is_some_and(|oldest| {
                // Same order as the fetch: time, then id (alarms share seconds).
                (record.occurred_at, &record.id) < (oldest.record.occurred_at, &oldest.record.id)
            })
    }

    #[must_use]
    pub fn newest_occurred_at(&self) -> Option<i64> {
        self.events
            .iter()
            .map(|entry| entry.record.occurred_at)
            .max()
    }

    #[must_use]
    pub const fn head(&self) -> u64 {
        self.head
    }

    /// Appends unknown alarms (oldest first) with optional decoded JPEGs.
    /// History-only batches (startup priming) raise the replay floor so
    /// cursors never receive them as new events.
    pub fn insert(
        &mut self,
        batch: Vec<(AlarmRecord, Option<Vec<u8>>)>,
        live: bool,
    ) -> Result<InsertOutcome, BridgeError> {
        let mut outcome = InsertOutcome::default();
        for (mut record, jpeg) in batch {
            if !super::model::is_token(&record.id)
                || self.knows(&record.id)
                || self.predates_full_history(&record)
            {
                continue;
            }
            record.has_picture = false;
            record.picture_bytes = 0;
            if let Some(jpeg) = jpeg {
                match self.pictures.write(&record.id, &jpeg) {
                    Ok(bytes) => {
                        record.has_picture = true;
                        record.picture_bytes = bytes;
                        outcome.pictures_stored += 1;
                    }
                    Err(_) => outcome.pictures_failed += 1,
                }
            }
            self.push(record);
            outcome.inserted += 1;
        }
        if !live {
            self.floor = self.head;
        }
        self.enforce_picture_budget();
        if outcome.inserted > 0 {
            self.persist()?;
        }
        Ok(outcome)
    }

    /// Drops the oldest pictures while the camera exceeds its byte budget;
    /// true when it dropped any.
    fn enforce_picture_budget(&mut self) -> bool {
        let mut total: u64 = self.events.iter().map(|e| e.record.picture_bytes).sum();
        let mut dropped = false;
        for entry in &mut self.events {
            if total <= self.picture_budget {
                break;
            }
            if entry.record.has_picture {
                self.pictures.remove(&entry.record.id);
                total = total.saturating_sub(entry.record.picture_bytes);
                entry.record.has_picture = false;
                entry.record.picture_bytes = 0;
                dropped = true;
            }
        }
        dropped
    }

    fn persist(&self) -> Result<(), BridgeError> {
        atomic_write_json(
            &self.file,
            &HistoryFile {
                schema_version: SCHEMA_VERSION,
                binding: self.binding.clone(),
                events: self
                    .events
                    .iter()
                    .map(|entry| entry.record.clone())
                    .collect(),
            },
        )
    }

    pub fn picture_bytes(&self, id: &str) -> Option<u64> {
        self.events
            .iter()
            .find(|entry| entry.record.id == id && entry.record.has_picture)
            .map(|entry| entry.record.picture_bytes)
    }
}
