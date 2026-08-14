use super::*;

fn options() -> CanaryOptions {
    CanaryOptions {
        base_url: "http://127.0.0.1:8765".into(),
        camera: "front-door".into(),
        seconds: 20,
        max_bytes: 64 * 1024 * 1024,
        stream_format: StreamFormat::Mpegps,
        skip_snapshot: false,
        token_file: None,
    }
}

#[test]
fn accepts_bounded_canary_options() {
    assert!(validate_options(&options()).is_ok());
}

#[test]
fn rejects_unsafe_camera_alias() {
    let mut candidate = options();
    candidate.camera = "front/door".into();
    assert!(validate_options(&candidate).is_err());
}

#[test]
fn rejects_unbounded_duration_and_size() {
    let mut candidate = options();
    candidate.seconds = 121;
    assert!(validate_options(&candidate).is_err());
    candidate.seconds = 20;
    candidate.max_bytes = MAX_STREAM_LIMIT + 1;
    assert!(validate_options(&candidate).is_err());
}

#[test]
fn temporary_media_names_are_unique() {
    assert_ne!(
        temporary_media_path(StreamFormat::Ts),
        temporary_media_path(StreamFormat::Ts)
    );
}
