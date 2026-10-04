//! History persistence, bounds, cursor semantics, long-poll and priming.

use super::JPEG;
use crate::{
    alarms::{AlarmRecord, MAX_EVENTS, store::CameraHistory},
    config::CameraConfig,
};

pub(super) fn camera(serial: &str) -> CameraConfig {
    CameraConfig {
        serial: serial.into(),
        channel: 1,
        substream: false,
        decrypt_video: false,
        media_key_file: None,
    }
}

pub(super) fn record(id: &str, at: i64) -> AlarmRecord {
    AlarmRecord {
        id: id.into(),
        occurred_at: at,
        alarm_type: 10_002,
        category: "motion".into(),
        title: "Motion".into(),
        has_picture: false,
        picture_bytes: 0,
    }
}

fn sequences(entries: &[crate::alarms::store::Entry]) -> Vec<u64> {
    entries.iter().map(|entry| entry.sequence).collect()
}

#[test]
fn history_round_trips_and_rejects_foreign_bindings() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("front");
    let mut history = CameraHistory::load(&path, &camera("CAM1"), 1 << 20);
    let batch = vec![
        (record("a", 1), Some(JPEG.to_vec())),
        (record("b", 2), None),
    ];
    let outcome = history
        .insert(batch, true)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!((outcome.inserted, outcome.pictures_stored), (2, 1));
    let reloaded = CameraHistory::load(&path, &camera("CAM1"), 1 << 20);
    let records: Vec<_> = reloaded.recent(10).into_iter().map(|e| e.record).collect();
    assert_eq!(records.len(), 2);
    assert!(records[0].has_picture && !records[1].has_picture);
    assert!(
        reloaded.after(0, 10).0.is_empty(),
        "reloaded history is not replayed"
    );
    assert!(path.join("pictures/a.jpg").is_file());
    let foreign = CameraHistory::load(&path, &camera("CAM2"), 1 << 20);
    assert_eq!(foreign.head(), 0);
    assert!(!path.join("pictures/a.jpg").exists());
}

#[test]
fn history_and_pictures_are_bounded() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("front");
    let budget = 3 * JPEG.len() as u64;
    let mut history = CameraHistory::load(&path, &camera("CAM1"), budget);
    let batch = (0..MAX_EVENTS + 5)
        .map(|index| (record(&format!("e{index}"), 10), Some(JPEG.to_vec())))
        .collect();
    history
        .insert(batch, true)
        .unwrap_or_else(|error| panic!("{error}"));
    let recent = history.recent(usize::MAX);
    assert_eq!(recent.len(), MAX_EVENTS);
    assert_eq!(recent[0].record.id, "e5");
    let with_pictures = recent
        .iter()
        .filter(|entry| entry.record.has_picture)
        .count();
    assert_eq!(with_pictures, 3);
    let files = std::fs::read_dir(path.join("pictures")).map_or(0, Iterator::count);
    assert_eq!(files, 3);
    assert!(
        path.join(format!("pictures/e{}.jpg", MAX_EVENTS + 4))
            .is_file()
    );
}

#[test]
fn cursor_reads_skip_primed_history_and_answer_stale_cursors() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let mut history = CameraHistory::load(&directory.path().join("c"), &camera("CAM1"), 1 << 20);
    let primed = vec![(record("p1", 1), None), (record("p2", 2), None)];
    history
        .insert(primed, false)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(history.after(0, 50).1, 2);
    assert!(history.after(0, 50).0.is_empty());
    let live = vec![
        (record("l1", 3), None),
        (record("l2", 4), None),
        (record("p1", 1), None),
    ];
    let outcome = history
        .insert(live, true)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(outcome.inserted, 2, "known IDs are deduplicated");
    let (events, next) = history.after(0, 50);
    assert_eq!((sequences(&events), next), (vec![3, 4], 4));
    let (events, next) = history.after(3, 1);
    assert_eq!((sequences(&events), next), (vec![4], 4));
    let (events, next) = history.after(4, 50);
    assert!(events.is_empty());
    assert_eq!(next, 4);
    let (events, next) = history.after(99, 50);
    assert!(events.is_empty());
    assert_eq!(next, 4);
    assert_eq!(sequences(&history.recent(3)), vec![2, 3, 4]);
}
