use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

fn render_cameras(root: &Path, input: &str) -> std::process::Output {
    let mut child = Command::new("jq")
        .args(["-S", "-e", "-f"])
        .arg(root.join("packaging/home-assistant/cameras.jq"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|error| panic!("jq is required by the add-on packaging test: {error}"));
    child
        .stdin
        .take()
        .unwrap_or_else(|| panic!("jq stdin was not piped"))
        .write_all(input.as_bytes())
        .unwrap_or_else(|error| panic!("write jq input: {error}"));
    child
        .wait_with_output()
        .unwrap_or_else(|error| panic!("wait for jq: {error}"))
}

fn read(root: &Path, relative: &str) -> String {
    fs::read_to_string(root.join(relative)).unwrap_or_else(|error| panic!("{relative}: {error}"))
}

fn assert_image_contract(root: &Path) {
    let dockerfile = fs::read_to_string(root.join("packaging/home-assistant/Dockerfile"))
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(dockerfile.contains("io.hass.type=\"app\""));
    assert!(dockerfile.contains("HEALTHCHECK"));
    assert!(dockerfile.contains("vistoda-app-bootstrap.sh"));
    assert!(!dockerfile.contains("8765:8765"));
}

fn assert_runtime_contract(root: &Path) {
    let runner = read(root, "packaging/home-assistant/run.sh");
    let bootstrap = read(root, "packaging/home-assistant/vistoda-app-bootstrap.sh");
    let camera_filter = read(root, "packaging/home-assistant/cameras.jq");
    assert!(bootstrap.contains("vistoda_supervisor_app_info"));
    assert!(runner.contains("vistoda_publish_app_discovery media_bridge 8765"));
    assert!(camera_filter.contains("cameras must contain 1 to 64 items"));
    assert!(camera_filter.contains("^[A-Za-z0-9_-]{1,64}$"));
    assert!(camera_filter.contains("aliases are not unique"));
    assert!(camera_filter.contains("camera sources are not unique"));
    assert!(runner.contains("mv -f \"${cameras_tmp_file}\" \"${cameras_file}\""));
    assert!(runner.contains("aliases: $aliases, devices: $devices"));
    assert!(bootstrap.contains("managed_app: true"));
    assert!(runner.contains("source_id:"));
    assert!(bootstrap.contains("http://supervisor/discovery"));
    assert!(bootstrap.contains("http://supervisor/addons/self/info"));
    assert!(runner.contains("--rawfile api_token"));
    assert!(runner.contains("vistoda_prepare_data_dir bridge:bridge \"${data_dir}\""));
    assert!(runner.contains("vistoda_secure_file bridge:bridge \"${data_dir}/token.json\""));
    assert!(camera_filter.contains("substream values are invalid"));
    assert!(!camera_filter.contains(".substream // false | booleans"));
    assert!(camera_filter.contains("verification codes are invalid"));
    assert!(runner.contains("readonly keys_dir=/data/keys"));
    assert!(runner.contains("mkdir -m 0700 \"${keys_dir}\""));
    assert!(runner.contains("chmod 0600 \"${key_tmp}\""));
}

fn assert_release_contract(root: &Path) {
    let workflow = read(root, ".github/workflows/publish-addon.yaml");
    let api = read(root, "openapi.yaml");
    assert!(workflow.contains("[\"amd64\", \"aarch64\"]"));
    assert!(workflow.contains("home-assistant/builder/actions/build-image"));
    assert!(workflow.contains("publish-multi-arch-manifest"));
    assert!(api.contains("/v1/enrollments:"));
    assert!(api.contains("/v1/enrollments/{enrollment_id}:"));
    assert!(api.contains("/v1/cameras/{camera}/identity:"));
    assert!(api.contains("#/components/parameters/ExpectedBinding"));
}

#[test]
fn home_assistant_app_is_private_discovered_and_multiarch() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    assert_image_contract(&root);
    assert_runtime_contract(&root);
    assert_release_contract(&root);
}

#[test]
fn camera_renderer_executes_legacy_and_multi_camera_contracts() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let legacy = render_cameras(
        &root,
        r#"{"alias":"front","camera_serial":"ABC123","camera_channel":2,"substream":false}"#,
    );
    assert!(
        legacy.status.success(),
        "{}",
        String::from_utf8_lossy(&legacy.stderr)
    );
    assert!(String::from_utf8_lossy(&legacy.stdout).contains("\"front\""));

    let multi = render_cameras(
        &root,
        r#"{"cameras":[{"alias":"front","serial":"ABC123","channel":1,"substream":false},{"alias":"rear","serial":"DEF456","channel":2,"substream":true}]}"#,
    );
    assert!(
        multi.status.success(),
        "{}",
        String::from_utf8_lossy(&multi.stderr)
    );
    let output = String::from_utf8_lossy(&multi.stdout);
    assert!(output.contains("\"front\"") && output.contains("\"rear\""));

    let duplicate = render_cameras(
        &root,
        r#"{"cameras":[{"alias":"one","serial":"ABC123","channel":1},{"alias":"two","serial":"ABC123","channel":1}]}"#,
    );
    assert!(!duplicate.status.success());
    assert!(String::from_utf8_lossy(&duplicate.stderr).contains("camera sources are not unique"));
}

#[test]
fn camera_renderer_maps_verification_codes_to_private_key_files() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let output = render_cameras(
        &root,
        r#"{"cameras":[{"alias":"front","serial":"ABC123","verification_code":"ABCDEF"},{"alias":"rear","serial":"DEF456","channel":2,"verification_code":""}]}"#,
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let rendered: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}"));
    // A code never forces decryption; the bridge follows the cloud flag.
    assert_eq!(rendered["front"]["decrypt_video"], false);
    assert_eq!(rendered["front"]["media_key_file"], "/data/keys/front.code");
    assert_eq!(rendered["rear"]["decrypt_video"], false);
    assert!(rendered["rear"].get("media_key_file").is_none());
    // The code itself never reaches cameras.json.
    assert!(!String::from_utf8_lossy(&output.stdout).contains("ABCDEF"));
    for invalid in ["AB C", "ABC", "\u{e9}\u{e9}\u{e9}\u{e9}"] {
        let rejected = render_cameras(
            &root,
            &format!(
                r#"{{"cameras":[{{"alias":"front","serial":"ABC123","verification_code":"{invalid}"}}]}}"#
            ),
        );
        assert!(!rejected.status.success(), "{invalid}");
        assert!(
            String::from_utf8_lossy(&rejected.stderr).contains("verification codes are invalid")
        );
    }
}

/// Runs the discovery tail of run.sh with the vendored kit and pins the
/// payload Home Assistant receives: keys and values match the 0.10 payload.
#[test]
fn supervisor_discovery_payload_is_unchanged() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let runner = read(&root, "packaging/home-assistant/run.sh");
    let discovery = runner
        .split_once("# The kit adds url")
        .unwrap_or_else(|| panic!("discovery block"))
        .1
        .split("\nvistoda_wait_child")
        .next()
        .unwrap_or_else(|| panic!("discovery tail"));
    let directory = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
    let cameras = directory.path().join("cameras.json");
    fs::write(
        &cameras,
        r#"{"front":{"serial":"S1","channel":1},"rear":{"serial":"S2","channel":2}}"#,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    let token = directory.path().join("api-token");
    fs::write(&token, format!("{}\n", "a".repeat(64))).unwrap_or_else(|error| panic!("{error}"));
    let script = format!(
        ". \"$BOOTSTRAP\"\n\
         vistoda_supervisor_app_info() {{ printf '{{\"data\":{{\"hostname\":\"fixture-ezviz\"}}}}'; }}\n\
         vistoda_publish_discovery() {{ cat; }}\n#{discovery}"
    );
    let output = Command::new("sh")
        .args(["-eu", "-c", &script])
        .env(
            "BOOTSTRAP",
            root.join("packaging/home-assistant/vistoda-app-bootstrap.sh"),
        )
        .env("cameras_file", &cameras)
        .env("token_file", &token)
        .env("camera_alias", "front")
        .env(
            "camera_alias_json",
            r#"[{"alias":"front","source_id":"S1:1"},{"alias":"rear","source_id":"S2:2"}]"#,
        )
        .output()
        .unwrap_or_else(|error| panic!("sh: {error}"));
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let payload: serde_json::Value =
        serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        payload,
        serde_json::json!({"service": "media_bridge", "config": {
            "provider": "ezviz", "url": "http://fixture-ezviz:8765", "alias": "front",
            "aliases": ["front", "rear"],
            "devices": [{"alias": "front", "source_id": "S1:1"},
                        {"alias": "rear", "source_id": "S2:2"}],
            "api_token": "a".repeat(64), "managed_app": true}})
    );
}
