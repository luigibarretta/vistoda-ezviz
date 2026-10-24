//! microSD intervals become ordinary recordings, or fail without leaving media.

mod sd_support;

use ezviz_vtm_bridge::error::BridgeError;
use sd_support::{Outcome, PS, interval, media_files, settled, system};

#[tokio::test]
async fn an_interval_becomes_a_ready_recording_with_its_bounds() {
    let system = system(Outcome::Clip);
    let queued = system
        .manager
        .start_sd("front", interval(1_000, 1_030), "sd-test-0001")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(queued.source.as_deref(), Some("sd"));
    assert_eq!(queued.requested_duration_seconds, 30);
    let ready = settled(&system.manager, &queued.recording_id).await;
    assert_eq!(ready.status, "ready", "{:?}", ready.error_code);
    assert_eq!((ready.sd_start, ready.sd_end), (Some(1_000), Some(1_030)));
    assert_eq!(ready.media_type, "video/mpeg");
    assert_eq!(ready.bytes, Some((PS.len() * 8) as u64));
    assert_eq!(ready.actual_duration_seconds, Some(30.0));
    assert_eq!(
        *system
            .fake
            .intervals
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        [("L1000".to_owned(), "L1030".to_owned())]
    );
    let files = media_files(&system.path);
    assert_eq!(files, [format!("{}.mpegps", ready.recording_id)]);
    // An idempotent retry returns the same manifest; another interval is refused.
    let again = system
        .manager
        .start_sd("front", interval(1_000, 1_030), "sd-test-0001")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(again.recording_id, ready.recording_id);
    assert!(matches!(
        system
            .manager
            .start_sd("front", interval(1_000, 1_031), "sd-test-0001")
            .await,
        Err(BridgeError::Recording(_))
    ));
}

#[tokio::test]
async fn an_empty_interval_fails_as_sd_no_recording_without_media() {
    let system = system(Outcome::NoRecording);
    let queued = system
        .manager
        .start_sd("front", interval(2_000, 2_010), "sd-test-0002")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let failed = settled(&system.manager, &queued.recording_id).await;
    assert_eq!(failed.status, "failed");
    assert_eq!(failed.error_code.as_deref(), Some("sd_no_recording"));
    assert!(media_files(&system.path).is_empty());
}

#[tokio::test]
async fn a_session_error_after_media_never_publishes_a_partial_clip() {
    let system = system(Outcome::OfflineAfterMedia);
    let queued = system
        .manager
        .start_sd("front", interval(3_000, 3_010), "sd-test-0003")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    let failed = settled(&system.manager, &queued.recording_id).await;
    assert_eq!(failed.error_code.as_deref(), Some("camera_offline"));
    assert!(
        media_files(&system.path).is_empty(),
        "partial media was kept"
    );
}

#[tokio::test]
async fn live_view_and_bounds_refuse_a_new_interval() {
    let system = system(Outcome::Clip);
    for (start, end) in [(10, 10), (10, 9), (10, 131)] {
        assert!(matches!(
            system
                .manager
                .start_sd("front", interval(start, end), "sd-test-0004")
                .await,
            Err(BridgeError::Recording(_))
        ));
    }
    let live = system
        .hub
        .subscribe()
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(matches!(
        system
            .manager
            .start_sd("front", interval(10, 20), "sd-test-0005")
            .await,
        Err(BridgeError::Capacity(_))
    ));
    system.hub.unsubscribe(live.id);
    system.hub.close().await;
    system.manager.close().await;
}

#[tokio::test]
async fn one_camera_runs_one_microsd_recording_at_a_time() {
    let system = system(Outcome::Hang);
    let first = system
        .manager
        .start_sd("front", interval(10, 20), "sd-test-0006")
        .await
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(first.is_active());
    assert!(matches!(
        system
            .manager
            .start_sd("front", interval(30, 40), "sd-test-0007")
            .await,
        Err(BridgeError::RecordingActive)
    ));
    system.manager.close().await;
}
