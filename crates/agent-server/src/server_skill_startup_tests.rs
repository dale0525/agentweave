use super::server_skill_startup::load_skill_manager;
use agent_runtime::platform::PlatformId;
use agent_runtime::skill_bundle::{BuildSkillBundleRequest, build_skill_bundle};
use agent_runtime::storage::Storage;
use std::path::Path;

struct TestDir(std::path::PathBuf);

impl TestDir {
    fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "general-agent-startup-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn startup_treats_lock_only_root_as_damaged_bundle() {
    assert_bundle_startup_error("skill-bundle.lock", "{}").await;
}

#[tokio::test]
async fn startup_treats_manifest_only_root_as_damaged_bundle() {
    assert_bundle_startup_error("skill-bundle.json", "{}").await;
}

#[tokio::test]
async fn startup_treats_generation_layout_without_current_as_damaged_bundle() {
    let temp = TestDir::new("generation-only");
    tokio::fs::create_dir_all(temp.path().join("generations"))
        .await
        .unwrap();

    let error = load_skill_manager(
        temp.path(),
        Storage::connect("sqlite::memory:").await.unwrap(),
        None,
    )
    .await
    .err()
    .unwrap();

    assert!(format!("{error:#}").contains("bundle current marker"));
}

#[tokio::test]
async fn deleting_bundle_metadata_never_downgrades_to_directory_discovery() {
    let temp = TestDir::new("metadata-deletion");
    let source = temp.path().join("source");
    let output = temp.path().join("bundle");
    write_package(&source.join("com.example.startup")).await;
    build_skill_bundle(BuildSkillBundleRequest {
        source_roots: vec![source],
        output_root: output.clone(),
        platform: PlatformId::Desktop,
        runtime_version: env!("CARGO_PKG_VERSION").parse().unwrap(),
        generated_at: "2026-07-12T00:00:00Z".into(),
    })
    .await
    .unwrap();
    tokio::fs::remove_file(output.join("current"))
        .await
        .unwrap();

    let error = load_skill_manager(
        &output,
        Storage::connect("sqlite::memory:").await.unwrap(),
        None,
    )
    .await
    .err()
    .unwrap();

    assert!(format!("{error:#}").contains("bundle current marker"));
}

#[tokio::test]
async fn deleting_manifest_and_lock_never_downgrades_to_directory_discovery() {
    let temp = TestDir::new("generation-metadata-deletion");
    let source = temp.path().join("source");
    let output = temp.path().join("bundle");
    write_package(&source.join("com.example.startup")).await;
    build_skill_bundle(BuildSkillBundleRequest {
        source_roots: vec![source],
        output_root: output.clone(),
        platform: PlatformId::Desktop,
        runtime_version: env!("CARGO_PKG_VERSION").parse().unwrap(),
        generated_at: "2026-07-12T00:00:00Z".into(),
    })
    .await
    .unwrap();
    let current: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(output.join("current")).await.unwrap()).unwrap();
    let generation = output
        .join("generations")
        .join(current["generation"].as_str().unwrap());
    tokio::fs::remove_file(generation.join("skill-bundle.json"))
        .await
        .unwrap();
    tokio::fs::remove_file(generation.join("skill-bundle.lock"))
        .await
        .unwrap();

    let error = load_skill_manager(
        &output,
        Storage::connect("sqlite::memory:").await.unwrap(),
        None,
    )
    .await
    .err()
    .unwrap();

    assert!(format!("{error:#}").contains("bundle metadata"));
}

async fn assert_bundle_startup_error(name: &str, contents: &str) {
    let temp = TestDir::new("single-evidence");
    tokio::fs::write(temp.path().join(name), contents)
        .await
        .unwrap();

    let result = load_skill_manager(
        temp.path(),
        Storage::connect("sqlite::memory:").await.unwrap(),
        None,
    )
    .await;

    assert!(
        result.is_err(),
        "{name} unexpectedly downgraded to directory mode"
    );
}

async fn write_package(root: &Path) {
    tokio::fs::create_dir_all(root).await.unwrap();
    tokio::fs::write(
        root.join("general-agent.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "id": "com.example.startup",
            "version": "0.1.0",
            "displayName": "Startup",
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
            "name": "startup", "description": "Startup fixture.", "version": "0.1.0",
            "entry": { "type": "command", "command": "node", "args": ["index.js"] },
            "tools": [{ "name": "run", "description": "Run.", "input_schema": {"type":"object"} }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(root.join("index.js"), "process.stdout.write('{}')\n")
        .await
        .unwrap();
}
