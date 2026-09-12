use std::{fs, path::Path};

#[test]
fn both_images_preserve_adapted_code_license_and_runtime_inventory() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let license = fs::read_to_string(root.join("third_party/cloud-cam-viewer/LICENSE"))
        .unwrap_or_else(|error| panic!("upstream license: {error}"));
    assert!(license.contains("Copyright (c) 2026 Cloud CCTV Viewer contributors"));
    assert!(license.contains("Permission is hereby granted, free of charge"));
    assert!(license.contains("THE SOFTWARE IS PROVIDED \"AS IS\""));
    for relative in ["Dockerfile", "packaging/home-assistant/Dockerfile"] {
        let recipe = fs::read_to_string(root.join(relative))
            .unwrap_or_else(|error| panic!("{relative}: {error}"));
        assert!(recipe.contains("COPY third_party /usr/share/doc/vistoda/third_party"));
        assert!(recipe.contains("COPY DISTRIBUTION.md /usr/share/doc/vistoda/"));
        assert!(recipe.contains("COPY --from=corresponding_sources /corresponding-source.tar"));
        assert!(recipe.contains(
            "RUN sh /usr/local/lib/vistoda-collect-runtime-inventory /usr/share/doc/vistoda/runtime"
        ));
    }
    let notice =
        fs::read_to_string(root.join("NOTICE")).unwrap_or_else(|error| panic!("NOTICE: {error}"));
    assert!(!notice.contains("independent Rust implementation"));
    assert!(notice.contains("third_party/cloud-cam-viewer/LICENSE"));
}
