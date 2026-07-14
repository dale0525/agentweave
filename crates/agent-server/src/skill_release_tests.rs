use super::skill_release::{gate_release_validation_after_discovery, validate_skill_roots};
use std::path::{Path, PathBuf};

#[tokio::test]
async fn release_validation_uses_one_secure_snapshot_after_package_replacement() {
    let temp = TestDir::new();
    let root = temp.path().join("skills");
    let package = root.join("provider");
    write_runtime_package(&package).await;
    let canonical_package = tokio::fs::canonicalize(&package).await.unwrap();
    let gate = gate_release_validation_after_discovery(&canonical_package);
    let validating_root = root.clone();
    let validating = tokio::spawn(async move { validate_skill_roots(&[validating_root]).await });
    gate.wait_entered().await;
    let displaced = temp.path().join("displaced-provider");
    tokio::fs::rename(&package, &displaced).await.unwrap();
    tokio::fs::create_dir_all(&package).await.unwrap();
    tokio::fs::write(package.join("skill.json"), b"{")
        .await
        .unwrap();
    tokio::fs::write(package.join("SKILL.md"), b"replacement")
        .await
        .unwrap();
    gate.release().await;

    let report = validating.await.unwrap();

    assert!(report.is_ready(), "{:?}", report.errors);
    assert_eq!(report.package_count, 1);
}

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "agentweave-release-snapshot-{}",
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

async fn write_runtime_package(root: &Path) {
    tokio::fs::create_dir_all(root).await.unwrap();
    tokio::fs::write(
        root.join("agentweave.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "id": "com.example.release-snapshot",
            "version": "0.1.0",
            "displayName": "Release Snapshot",
            "kind": "native_runtime",
            "package": { "includeInstructions": false, "includeRuntime": true },
            "compatibility": { "platforms": ["desktop", "server"] },
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
            "name": "release-snapshot",
            "description": "Release snapshot fixture.",
            "version": "0.1.0",
            "entry": { "type": "command", "command": "node", "args": ["index.js"] },
            "tools": [{
                "name": "read", "description": "Read.",
                "input_schema": { "type": "object" }
            }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(root.join("index.js"), "process.stdout.write('{}')\n")
        .await
        .unwrap();
}
