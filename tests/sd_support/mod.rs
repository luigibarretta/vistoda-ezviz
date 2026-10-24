//! Shared fixtures for the microSD recording tests.

use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use async_trait::async_trait;
use ezviz_vtm_bridge::{
    config::CameraConfig,
    error::BridgeError,
    hub::RawStreamHub,
    metrics::Metrics,
    recordings::{RecordingManager, RecordingManifest, SdInterval},
    transport::{CameraTransport, ChunkConsumer},
};
use tempfile::TempDir;
use tokio::time::{Instant, sleep};
use tokio_util::sync::CancellationToken;

pub const PS: &[u8] = b"\x00\x00\x01\xba-sd-media";

#[derive(Clone, Copy)]
pub enum Outcome {
    Clip,
    NoRecording,
    OfflineAfterMedia,
    /// Holds the session open until cancelled.
    Hang,
}

pub struct Fake {
    outcome: Outcome,
    pub intervals: Mutex<Vec<(String, String)>>,
}

#[async_trait]
impl CameraTransport for Fake {
    async fn snapshot_jpeg(&self, _: &CameraConfig) -> Result<Vec<u8>, BridgeError> {
        Ok(Vec::new())
    }

    async fn stream_mpeg_ps(
        &self,
        _: &CameraConfig,
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        while !cancel.is_cancelled() && output.consume(PS.to_vec()) {
            sleep(Duration::from_millis(10)).await;
        }
        Ok(())
    }

    async fn stream_sd_playback(
        &self,
        _: &CameraConfig,
        (begin, end): (&str, &str),
        cancel: CancellationToken,
        output: Arc<dyn ChunkConsumer>,
    ) -> Result<(), BridgeError> {
        self.intervals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push((begin.to_owned(), end.to_owned()));
        match self.outcome {
            Outcome::NoRecording => Err(BridgeError::SdNoRecording),
            Outcome::Hang => {
                cancel.cancelled().await;
                Ok(())
            }
            Outcome::Clip | Outcome::OfflineAfterMedia => {
                for _ in 0..8 {
                    output.consume(PS.to_vec());
                }
                match self.outcome {
                    Outcome::OfflineAfterMedia => Err(BridgeError::CameraOffline),
                    _ => Ok(()),
                }
            }
        }
    }
}

pub struct System {
    _directory: TempDir,
    pub path: std::path::PathBuf,
    pub hub: Arc<RawStreamHub>,
    pub fake: Arc<Fake>,
    pub manager: Arc<RecordingManager>,
}

pub fn system(outcome: Outcome) -> System {
    let directory = TempDir::new().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("spool");
    let fake = Arc::new(Fake {
        outcome,
        intervals: Mutex::new(Vec::new()),
    });
    let camera = CameraConfig {
        serial: "hidden".into(),
        channel: 1,
        substream: false,
        decrypt_video: false,
        media_key_file: None,
    };
    let transport: Arc<dyn CameraTransport> = fake.clone();
    let hub = RawStreamHub::new(
        "front".into(),
        camera,
        transport,
        Metrics::default(),
        8,
        2,
        Duration::ZERO,
    );
    let hubs = BTreeMap::from([("front".to_owned(), Arc::clone(&hub))]);
    let manager = RecordingManager::load(
        path.clone(),
        hubs,
        Metrics::default(),
        120,
        1 << 20,
        1 << 24,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    System {
        _directory: directory,
        path,
        hub,
        fake,
        manager,
    }
}

pub fn interval(start: i64, end: i64) -> SdInterval {
    SdInterval {
        start,
        end,
        begin_local: format!("L{start}"),
        end_local: format!("L{end}"),
    }
}

pub async fn settled(manager: &RecordingManager, id: &str) -> RecordingManifest {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let manifest = manager
            .get(id)
            .await
            .unwrap_or_else(|| panic!("manifest disappeared"));
        if !manifest.is_active() {
            return manifest;
        }
        assert!(Instant::now() < deadline, "recording did not settle");
        sleep(Duration::from_millis(20)).await;
    }
}

pub fn media_files(path: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(path)
        .unwrap_or_else(|error| panic!("{error}"))
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name != "recordings.json")
        .collect()
}
