//! `--sd-day`: bounded runs, the ledger and retries after a failed recording.

use serde_json::json;

use super::*;

fn day(max_clips: usize) -> sd::SdDay {
    sd::SdDay {
        day: "2026-10-24".into(),
        max_clips,
        max_seconds: 120,
    }
}

fn bodies(state: &MockState) -> Vec<(i64, i64)> {
    state
        .bodies
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .iter()
        .map(|body| {
            (
                body["start"].as_i64().unwrap_or(-1),
                body["end"].as_i64().unwrap_or(-1),
            )
        })
        .collect()
}

#[tokio::test]
async fn a_microsd_day_is_imported_once_even_after_a_failed_attempt() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let state = Arc::new(MockState::default());
    let (base, task) = server(Arc::clone(&state)).await;
    let template = request(directory.path(), base, "");
    let mut counts = Vec::new();
    let mut first_path = None;
    for max_clips in [1, 5, 5, 5] {
        let pulled = sd::pull_day(&template, &day(max_clips))
            .await
            .unwrap_or_else(|error| panic!("{error}"));
        first_path = first_path.or_else(|| pulled.first().map(|item| item.path.clone()));
        counts.push(pulled.len());
    }
    // Run 1 imports the oldest clip; run 2 meets the asleep camera on the long
    // clip and continues; run 3 retries it with a new key; run 4 has nothing.
    assert_eq!(counts, [1, 0, 1, 0]);
    assert_eq!(bodies(&state), [(100, 130), (200, 320), (200, 320)]);
    // Named after the recording time (epoch 100 s), not the download time.
    let path = first_path.unwrap_or_default();
    assert!(path.to_string_lossy().contains("hiv19700101T000140Z-"));
    let ledger: Value = serde_json::from_slice(
        &fs::read(directory.path().join("media/.sd-ledger.json"))
            .unwrap_or_else(|error| panic!("{error}")),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        ledger,
        json!({"done": ["sd-front-100-130", "sd-front-200-600"], "attempts": {}})
    );
    task.abort();
}

#[tokio::test]
async fn a_clip_already_in_the_destination_is_not_imported_again() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let state = Arc::new(MockState::default());
    let (base, task) = server(Arc::clone(&state)).await;
    let template = request(directory.path(), base, "");
    // A crash after the bridge ACK left the file but not the ledger entry.
    fs::create_dir_all(&template.destination).unwrap_or_else(|error| panic!("{error}"));
    fs::write(
        template
            .destination
            .join("hiv19700101T000140Z-earlier.mpegps"),
        MEDIA,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let pulled = sd::pull_day(&template, &day(1))
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    // The 100 s clip is skipped without a request; the attempt goes to 200 s.
    assert!(pulled.is_empty());
    assert_eq!(bodies(&state), [(200, 320)]);
    task.abort();
}
