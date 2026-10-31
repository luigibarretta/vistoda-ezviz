//! Configured history limit, eviction index and per-camera picture budget.

use std::collections::BTreeMap;

use super::{
    JPEG,
    feed::{camera, record},
};
use crate::{
    alarms::{AlarmFeed, AlarmLimits, store::CameraHistory},
    metrics::Metrics,
};

fn limited(path: &std::path::Path, limit: usize) -> CameraHistory {
    CameraHistory::load(path, &camera("CAM1"), 1 << 20, limit)
}

fn ids(history: &CameraHistory) -> Vec<String> {
    history
        .recent(usize::MAX)
        .into_iter()
        .map(|entry| entry.record.id)
        .collect()
}

#[test]
fn lowering_the_limit_trims_at_load() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("front");
    let batch = (1..=5)
        .map(|at| (record(&format!("e{at}"), at), Some(JPEG.to_vec())))
        .collect();
    limited(&path, 5)
        .insert(batch, true)
        .unwrap_or_else(|error| panic!("{error}"));
    let reloaded = limited(&path, 2);
    assert_eq!(ids(&reloaded), ["e4", "e5"]);
    assert!(
        !path.join("pictures/e1.jpg").exists(),
        "evicted pictures are deleted"
    );
    assert!(path.join("pictures/e5.jpg").is_file());
}

#[test]
fn evicted_alarms_are_not_reinserted() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let mut history = limited(&directory.path().join("front"), 2);
    let batch = vec![
        (record("a", 1), None),
        (record("b", 2), None),
        (record("c", 3), None),
    ];
    history
        .insert(batch, true)
        .unwrap_or_else(|error| panic!("{error}"));
    let again = history
        .insert(vec![(record("a", 1), None)], true)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(again.inserted, 0, "older than the oldest retained alarm");
    let newer = history
        .insert(vec![(record("d", 4), None)], true)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(newer.inserted, 1);
    assert_eq!(ids(&history), ["c", "d"]);
}

#[test]
fn index_follows_eviction_and_reload() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("front");
    let mut history = limited(&path, 2);
    let batch = vec![
        (record("a", 1), None),
        (record("b", 2), None),
        (record("c", 3), None),
    ];
    history
        .insert(batch, true)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!history.knows("a") && history.knows("b") && history.knows("c"));
    let reloaded = limited(&path, 2);
    assert!(!reloaded.knows("a") && reloaded.knows("b") && reloaded.knows("c"));
}

#[tokio::test]
async fn picture_budget_is_split_per_camera() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let cameras = BTreeMap::from([
        ("front".to_owned(), camera("CAM1")),
        ("back".to_owned(), camera("CAM2")),
    ]);
    // Six pictures in total, so three per camera.
    let limits = AlarmLimits {
        history: 50,
        picture_bytes: 6 * JPEG.len() as u64,
    };
    let feed = AlarmFeed::load(directory.path(), &cameras, Metrics::default(), limits);
    let batch = (1..=5)
        .map(|at| (record(&format!("e{at}"), at), Some(JPEG.to_vec())))
        .collect();
    feed.publish("front", batch, true)
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let history = feed.cameras["front"].history.lock().await;
    let kept: Vec<_> = history
        .recent(usize::MAX)
        .into_iter()
        .filter(|entry| entry.record.has_picture)
        .map(|entry| entry.record.id)
        .collect();
    assert_eq!(
        kept,
        ["e3", "e4", "e5"],
        "oldest pictures go first, alarms stay"
    );
    assert_eq!((history.limit(), history.recent(usize::MAX).len()), (50, 5));
}

#[test]
fn lowering_the_limit_rewrites_the_history_file() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("front");
    let batch = (1..=5)
        .map(|at| (record(&format!("e{at}"), at), None))
        .collect();
    limited(&path, 5)
        .insert(batch, true)
        .unwrap_or_else(|error| panic!("{error}"));
    drop(limited(&path, 2));
    assert_eq!(
        ids(&limited(&path, 5)),
        ["e4", "e5"],
        "trimmed alarms must not come back when the limit is raised again"
    );
}

#[test]
fn an_evicted_alarm_of_the_same_second_stays_out() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let mut history = limited(&directory.path().join("front"), 2);
    let batch = vec![
        (record("a", 5), None),
        (record("b", 5), None),
        (record("c", 5), None),
    ];
    history
        .insert(batch, true)
        .unwrap_or_else(|error| panic!("{error}"));
    let again = history
        .insert(vec![(record("a", 5), None)], true)
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        again.inserted, 0,
        "same second, ordered before the oldest kept"
    );
    assert_eq!(ids(&history), ["b", "c"]);
}

#[test]
fn a_lowered_picture_budget_is_written_back_at_load() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let path = directory.path().join("front");
    let batch = (1..=3)
        .map(|at| (record(&format!("e{at}"), at), Some(JPEG.to_vec())))
        .collect();
    CameraHistory::load(&path, &camera("CAM1"), 1 << 20, 10)
        .insert(batch, true)
        .unwrap_or_else(|error| panic!("{error}"));
    drop(CameraHistory::load(
        &path,
        &camera("CAM1"),
        JPEG.len() as u64,
        10,
    ));
    let stored = std::fs::read_to_string(path.join("history.json"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(stored.matches("\"has_picture\":true").count(), 1);
}
