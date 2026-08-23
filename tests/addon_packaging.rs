use std::{fs, path::PathBuf};

#[test]
fn home_assistant_app_is_private_discovered_and_multiarch() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let dockerfile = fs::read_to_string(root.join("packaging/home-assistant/Dockerfile"))
        .unwrap_or_else(|error| panic!("{error}"));
    let runner = fs::read_to_string(root.join("packaging/home-assistant/run.sh"))
        .unwrap_or_else(|error| panic!("{error}"));
    let workflow = fs::read_to_string(root.join(".github/workflows/publish-addon.yaml"))
        .unwrap_or_else(|error| panic!("{error}"));
    let api =
        fs::read_to_string(root.join("openapi.yaml")).unwrap_or_else(|error| panic!("{error}"));
    assert!(dockerfile.contains("io.hass.type=\"app\""));
    assert!(dockerfile.contains("HEALTHCHECK"));
    assert!(runner.contains("http://supervisor/discovery"));
    assert!(runner.contains("http://supervisor/addons/self/info"));
    assert!(runner.contains("--rawfile api_token"));
    assert!(runner.contains("managed_app: true"));
    assert!(runner.contains("chown bridge:bridge \"${data_dir}\""));
    assert!(runner.contains("chmod 0600 \"${data_dir}/token.json\""));
    assert!(!runner.contains("8765:8765"));
    assert!(workflow.contains("[\"amd64\", \"aarch64\"]"));
    assert!(workflow.contains("home-assistant/builder/actions/build-image"));
    assert!(workflow.contains("publish-multi-arch-manifest"));
    assert!(api.contains("/v1/enrollments:"));
    assert!(api.contains("/v1/enrollments/{enrollment_id}:"));
}
