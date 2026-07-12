use crate::platform::PlatformId;
use crate::skill_bundle::{
    BuildSkillBundleRequest, BundleSkillSource, build_skill_bundle, build_skill_bundle_with_faults,
    gate_bundle_after_final_validation, gate_bundle_before_publish,
    gate_bundle_discovery_after_layout,
};
use crate::skill_source::SkillSource;
use crate::skill_store_faults::{StoreFaultPoint, StoreFaults};
use semver::Version;
use std::io::{Seek, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(unix)]
#[tokio::test]
async fn generation_replacement_after_final_validation_falls_back_to_previous_commitment() {
    let fixture = FinalReviewFixture::new().await;
    build_skill_bundle(fixture.request()).await.unwrap();
    let old = selected_package(&fixture.output).await;
    fixture.write_runtime("new").await;

    let gate = gate_bundle_after_final_validation(&fixture.output);
    let request = fixture.request();
    let publishing = tokio::spawn(async move { build_skill_bundle(request).await });
    let generation = gate.wait_entered().await;
    let displaced = generation.with_file_name(uuid::Uuid::new_v4().to_string());
    tokio::fs::rename(&generation, &displaced).await.unwrap();
    tokio::fs::create_dir(&generation).await.unwrap();
    tokio::fs::write(generation.join("attacker"), "replacement")
        .await
        .unwrap();
    gate.release().await;
    let _ = publishing.await.unwrap();

    let current = read_current(&fixture.output).await;
    assert_ne!(
        current["active"]["generation"],
        current["previous"]["generation"]
    );
    assert_eq!(selected_package(&fixture.output).await, old);
    assert_eq!(
        tokio::fs::read_to_string(old.root.join("index.js"))
            .await
            .unwrap(),
        "old\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn preopened_writer_after_final_validation_falls_back_to_previous_commitment() {
    let fixture = FinalReviewFixture::new().await;
    build_skill_bundle(fixture.request()).await.unwrap();
    let old = selected_package(&fixture.output).await;
    fixture.write_runtime("new").await;

    let before = gate_bundle_before_publish(&fixture.output);
    let final_gate = gate_bundle_after_final_validation(&fixture.output);
    let request = fixture.request();
    let publishing = tokio::spawn(async move { build_skill_bundle(request).await });
    let generation = before.wait_entered().await;
    let path = generation.join("com.example.final/index.js");
    let mut writer = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    before.release().await;
    final_gate.wait_entered().await;
    writer.seek(std::io::SeekFrom::Start(0)).unwrap();
    writer.write_all(b"tampered-after-validation\n").unwrap();
    writer.set_len(26).unwrap();
    writer.sync_all().unwrap();
    final_gate.release().await;
    let _ = publishing.await.unwrap();

    assert_eq!(selected_package(&fixture.output).await, old);
    assert_eq!(
        tokio::fs::read_to_string(old.root.join("index.js"))
            .await
            .unwrap(),
        "old\n"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn committed_generation_replacement_falls_back_to_explicit_previous_generation() {
    let fixture = FinalReviewFixture::new().await;
    build_skill_bundle(fixture.request()).await.unwrap();
    let old = selected_package(&fixture.output).await;
    fixture.write_runtime("new").await;
    build_skill_bundle(fixture.request()).await.unwrap();
    let current = read_current(&fixture.output).await;
    let active = fixture
        .output
        .join("generations")
        .join(current["active"]["generation"].as_str().unwrap());
    let displaced = active.with_file_name(uuid::Uuid::new_v4().to_string());
    tokio::fs::rename(&active, &displaced).await.unwrap();
    tokio::fs::create_dir(&active).await.unwrap();
    tokio::fs::write(active.join("attacker"), "replacement")
        .await
        .unwrap();

    let selected = selected_package(&fixture.output).await;

    assert_eq!(selected, old);
    let expected = tokio::fs::canonicalize(
        fixture
            .output
            .join("generations")
            .join(current["previous"]["generation"].as_str().unwrap())
            .join("com.example.final"),
    )
    .await
    .unwrap();
    assert_eq!(selected.root, expected);
}

#[tokio::test]
async fn first_build_reservation_failure_is_immediately_retryable() {
    assert_first_build_retry(StoreFaultPoint::BundleBeforeGenerationReservation).await;
}

#[tokio::test]
async fn first_build_copy_failure_is_immediately_retryable() {
    assert_first_build_retry(StoreFaultPoint::StagingCopyFile).await;
}

#[tokio::test]
async fn first_build_precommit_failure_is_immediately_retryable() {
    assert_first_build_retry(StoreFaultPoint::BundleBeforePublish).await;
}

#[tokio::test]
async fn current_marker_schema_is_versioned_closed_and_committed() {
    let versioned = FinalReviewFixture::new().await;
    build_skill_bundle(versioned.request()).await.unwrap();
    let current = read_current(&versioned.output).await;
    assert_eq!(current["schemaVersion"], 2);
    assert_eq!(
        current["active"]["manifestSha256"].as_str().unwrap().len(),
        64
    );
    assert_eq!(current["active"]["lockSha256"].as_str().unwrap().len(), 64);
    assert!(current["previous"].is_null());

    let unknown = FinalReviewFixture::new().await;
    build_skill_bundle(unknown.request()).await.unwrap();
    let mut current = read_current(&unknown.output).await;
    current["unexpected"] = serde_json::json!(true);
    write_current(&unknown.output, &current).await;
    let error = BundleSkillSource::open(&unknown.output).await.unwrap_err();
    assert!(format!("{error:#}").contains("unknown field"));

    let unsupported = FinalReviewFixture::new().await;
    build_skill_bundle(unsupported.request()).await.unwrap();
    let mut current = read_current(&unsupported.output).await;
    current["schemaVersion"] = serde_json::json!(1);
    write_current(&unsupported.output, &current).await;
    let error = BundleSkillSource::open(&unsupported.output)
        .await
        .unwrap_err();
    assert!(format!("{error:#}").contains("current schema version"));
}

#[cfg(unix)]
#[tokio::test]
async fn independent_discovery_gates_are_path_keyed_and_bounded() {
    let first = FinalReviewFixture::new().await;
    let second = FinalReviewFixture::new().await;
    build_skill_bundle(first.request()).await.unwrap();
    build_skill_bundle(second.request()).await.unwrap();
    let first_generation = active_generation(&first.output).await;
    let second_generation = active_generation(&second.output).await;
    let first_source = BundleSkillSource::open(&first.output).await.unwrap();
    let second_source = BundleSkillSource::open(&second.output).await.unwrap();
    let first_gate = gate_bundle_discovery_after_layout(&first_generation);
    let second_gate = gate_bundle_discovery_after_layout(&second_generation);
    let first_reload = tokio::spawn(async move { first_source.discover().await });
    let second_reload = tokio::spawn(async move { second_source.discover().await });

    tokio::time::timeout(TEST_TIMEOUT, async {
        tokio::join!(first_gate.wait_entered(), second_gate.wait_entered());
    })
    .await
    .expect("independent discovery gates did not both enter");
    tokio::join!(first_gate.release(), second_gate.release());

    assert_eq!(first_reload.await.unwrap().unwrap().len(), 1);
    assert_eq!(second_reload.await.unwrap().unwrap().len(), 1);
}

async fn assert_first_build_retry(point: StoreFaultPoint) {
    let fixture = FinalReviewFixture::new().await;
    let faults = StoreFaults::default();
    faults.fail_once(point);

    build_skill_bundle_with_faults(fixture.request(), faults)
        .await
        .unwrap_err();

    assert!(
        !fixture.output.exists(),
        "failed bootstrap evidence remained"
    );
    build_skill_bundle(fixture.request()).await.unwrap();
    let package = selected_package(&fixture.output).await;
    assert_eq!(package.content, "old\n");
}

#[derive(Debug, PartialEq, Eq)]
struct SelectedPackage {
    root: PathBuf,
    hash: String,
    content: String,
}

async fn selected_package(output: &Path) -> SelectedPackage {
    let source = BundleSkillSource::open(output).await.unwrap();
    let package = source.packages().await.unwrap().remove(0);
    SelectedPackage {
        content: tokio::fs::read_to_string(package.root.join("index.js"))
            .await
            .unwrap(),
        root: package.root,
        hash: package.content_hash,
    }
}

async fn read_current(output: &Path) -> serde_json::Value {
    serde_json::from_slice(&tokio::fs::read(output.join("current")).await.unwrap()).unwrap()
}

async fn write_current(output: &Path, value: &serde_json::Value) {
    let mut bytes = serde_json::to_vec_pretty(value).unwrap();
    bytes.push(b'\n');
    tokio::fs::write(output.join("current"), bytes)
        .await
        .unwrap();
}

async fn active_generation(output: &Path) -> PathBuf {
    let current = read_current(output).await;
    tokio::fs::canonicalize(
        output
            .join("generations")
            .join(current["active"]["generation"].as_str().unwrap()),
    )
    .await
    .unwrap()
}

struct FinalReviewFixture {
    _temp: tempfile::TempDir,
    source: PathBuf,
    output: PathBuf,
}

impl FinalReviewFixture {
    async fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let output = temp.path().join("bundle");
        write_package(&source.join("com.example.final")).await;
        Self {
            _temp: temp,
            source,
            output,
        }
    }

    fn request(&self) -> BuildSkillBundleRequest {
        BuildSkillBundleRequest {
            source_roots: vec![self.source.clone()],
            output_root: self.output.clone(),
            platform: PlatformId::Desktop,
            runtime_version: Version::new(0, 1, 0),
            generated_at: "2026-07-12T00:00:00Z".into(),
        }
    }

    async fn write_runtime(&self, content: &str) {
        tokio::fs::write(
            self.source.join("com.example.final/index.js"),
            format!("{content}\n"),
        )
        .await
        .unwrap();
    }
}

async fn write_package(root: &Path) {
    tokio::fs::create_dir_all(root).await.unwrap();
    tokio::fs::write(
        root.join("general-agent.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "id": "com.example.final",
            "version": "0.1.0",
            "displayName": "Final",
            "kind": "native_runtime",
            "package": { "includeInstructions": false, "includeRuntime": true },
            "compatibility": { "platforms": ["desktop"] },
            "requires": {
                "packages": [], "capabilities": ["shell.process"],
                "runtimeTools": [], "connectors": []
            }
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.join("skill.json"),
        serde_json::json!({
            "name": "final", "description": "Final fixture.", "version": "0.1.0",
            "entry": { "type": "command", "command": "node", "args": ["index.js"] },
            "tools": [{ "name": "run", "description": "Run.", "input_schema": {"type":"object"} }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(root.join("index.js"), "old\n")
        .await
        .unwrap();
}
