use crate::skill_package::SkillPackageId;
use crate::skill_resolver::{ResolvedSkillPackage, ResolvedSkillSet, SkillResolutionStatus};
use crate::skill_snapshot::SkillSnapshot;
use crate::skill_source::{ManagedSkillSource, SkillSource};
use crate::skill_state::{SkillLayerRecord, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use serde_json::json;
use std::path::Path;
use tempfile::{TempDir, tempdir};

struct VerifiedFixture {
    _app: TempDir,
    _cache: TempDir,
    state: SkillStateStore,
    store: SkillRevisionStore,
    source: ManagedSkillSource,
    faults: SkillStoreTestFaults,
}

impl VerifiedFixture {
    async fn new() -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let state = SkillStateStore::new(Storage::connect("sqlite::memory:").await.unwrap());
        let faults = SkillStoreTestFaults::default();
        let store = SkillRevisionStore::with_test_faults(
            paths,
            state.clone(),
            SkillStoreLimits::default(),
            faults.clone(),
        );
        let source = ManagedSkillSource::from_store(store.clone());
        Self {
            _app: app,
            _cache: cache,
            state,
            store,
            source,
            faults,
        }
    }

    async fn active_package(
        &self,
        id: &str,
        package: TempDir,
    ) -> crate::skill_store::StoredSkillRevision {
        let staged = self
            .store
            .create_staging_revision(package.path(), "owner-1")
            .await
            .unwrap();
        let managed = self
            .store
            .promote_revision(&staged.revision_id)
            .await
            .unwrap();
        self.state
            .activate_revision(
                &SkillPackageId::parse(id).unwrap(),
                &managed.revision_id,
                SkillLayerRecord::Managed,
                "owner-1",
            )
            .await
            .unwrap();
        managed
    }
}

#[tokio::test]
async fn managed_snapshot_uses_discovery_verified_manifest_and_instructions_bytes() {
    let fixture = VerifiedFixture::new().await;
    let runtime = fixture
        .active_package(
            "com.example.verified-runtime",
            write_runtime_package("com.example.verified-runtime").await,
        )
        .await;
    let instructions = fixture
        .active_package(
            "com.example.verified-instructions",
            write_instruction_package("com.example.verified-instructions").await,
        )
        .await;
    let packages = fixture.source.discover().await.unwrap();
    make_writable(&runtime.path).await;
    make_writable(&instructions.path).await;
    tokio::fs::write(runtime.path.join("skill.json"), "{invalid")
        .await
        .unwrap();
    tokio::fs::write(instructions.path.join("SKILL.md"), "tampered")
        .await
        .unwrap();

    let snapshot = SkillSnapshot::build(1, active_set(packages)).await.unwrap();

    assert_eq!(snapshot.registry().tools()[0].name, "verified_tool");
    assert_eq!(
        snapshot.catalog().summaries()[0].name,
        "verified-instructions"
    );
    let documents = snapshot
        .catalog()
        .load_instruction_documents(&["verified-instructions".into()], usize::MAX)
        .await
        .unwrap();
    assert!(documents[0].content.contains("verified body"));
}

#[tokio::test]
async fn managed_execution_rehashes_tree_and_does_not_start_changed_command() {
    let fixture = VerifiedFixture::new().await;
    let managed = fixture
        .active_package(
            "com.example.verified-execution",
            write_runtime_package("com.example.verified-execution").await,
        )
        .await;
    let packages = fixture.source.discover().await.unwrap();
    let snapshot = SkillSnapshot::build(1, active_set(packages)).await.unwrap();
    make_writable(&managed.path).await;
    tokio::fs::write(
        managed.path.join("run.sh"),
        "printf started > marker\nprintf '{\"ok\":true}'\n",
    )
    .await
    .unwrap();

    let error = snapshot
        .registry()
        .execute("verified_tool", json!({}))
        .await
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(
        message.contains("managed execution snapshot hash mismatch"),
        "{message}"
    );
    assert!(!managed.path.join("marker").exists());
}

#[tokio::test]
async fn managed_execution_uses_private_snapshot_after_hash_to_spawn_mutation() {
    let fixture = VerifiedFixture::new().await;
    let managed = fixture
        .active_package(
            "com.example.execution-private",
            write_runtime_package("com.example.execution-private").await,
        )
        .await;
    let packages = fixture.source.discover().await.unwrap();
    let snapshot = SkillSnapshot::build(1, active_set(packages)).await.unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::ExecutionAfterSnapshot);
    let registry = snapshot.registry().clone();
    let execution = tokio::spawn(async move { registry.execute("verified_tool", json!({})).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), gate.wait_entered())
        .await
        .expect("execution did not reach private snapshot checkpoint");
    make_writable(&managed.path).await;
    tokio::fs::write(
        managed.path.join("run.sh"),
        "printf mutated > original-marker\nprintf '{\"ok\":false}'\n",
    )
    .await
    .unwrap();
    gate.release().await;

    let value = execution.await.unwrap().unwrap();
    assert_eq!(value, json!({"ok": true}));
    assert!(!managed.path.join("original-marker").exists());
}

#[tokio::test]
async fn quarantined_managed_residue_is_rejected_by_old_snapshot() {
    let fixture = VerifiedFixture::new().await;
    let managed = fixture
        .active_package(
            "com.example.execution-quarantined",
            write_runtime_package("com.example.execution-quarantined").await,
        )
        .await;
    let packages = fixture.source.discover().await.unwrap();
    let snapshot = SkillSnapshot::build(1, active_set(packages)).await.unwrap();
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::QuarantineSourceCleanup);
    fixture
        .store
        .quarantine_revision(&managed.revision_id, "security invalid")
        .await
        .unwrap();
    assert!(managed.path.is_dir());

    let error = snapshot
        .registry()
        .execute("verified_tool", json!({}))
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("no longer active managed revision"));
}

fn active_set(packages: Vec<crate::skill_source::DiscoveredSkillPackage>) -> ResolvedSkillSet {
    ResolvedSkillSet {
        active: packages
            .into_iter()
            .map(|package| ResolvedSkillPackage {
                package,
                status: SkillResolutionStatus::Active,
                reason: "active".into(),
            })
            .collect(),
        inactive: Vec::new(),
    }
}

async fn write_runtime_package(id: &str) -> TempDir {
    let root = tempdir().unwrap();
    let name = id.rsplit('.').next().unwrap();
    tokio::fs::write(
        root.path().join("general-agent.json"),
        json!({
            "schemaVersion": 1,
            "id": id,
            "version": "1.0.0",
            "displayName": name,
            "kind": "native_runtime",
            "package": {"includeInstructions": false, "includeRuntime": true}
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("skill.json"),
        json!({
            "name": name,
            "description": "verified runtime",
            "version": "1.0.0",
            "entry": {"type": "command", "command": "sh", "args": ["run.sh"]},
            "tools": [{
                "name": "verified_tool",
                "description": "verified tool",
                "input_schema": {"type": "object"}
            }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(root.path().join("run.sh"), "printf '{\"ok\":true}'\n")
        .await
        .unwrap();
    root
}

async fn write_instruction_package(id: &str) -> TempDir {
    let root = tempdir().unwrap();
    let name = id.rsplit('.').next().unwrap();
    tokio::fs::write(
        root.path().join("general-agent.json"),
        json!({
            "schemaVersion": 1,
            "id": id,
            "version": "1.0.0",
            "displayName": name,
            "kind": "instruction_only",
            "package": {"includeInstructions": true, "includeRuntime": false}
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("SKILL.md"),
        format!("---\nname: {name}\ndescription: verified instructions\n---\nverified body\n"),
    )
    .await
    .unwrap();
    root
}

async fn make_writable(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = tokio::fs::symlink_metadata(&path).await.unwrap();
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o200);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        tokio::fs::set_permissions(&path, permissions)
            .await
            .unwrap();
        if metadata.is_dir() {
            let mut entries = tokio::fs::read_dir(&path).await.unwrap();
            while let Some(entry) = entries.next_entry().await.unwrap() {
                stack.push(entry.path());
            }
        }
    }
}
