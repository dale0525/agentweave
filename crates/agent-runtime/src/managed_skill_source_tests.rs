use crate::skill_package::SkillPackageId;
use crate::skill_source::{ManagedSkillSource, SkillSource, hash_package_tree};
use crate::skill_state::{SkillLayerRecord, SkillStateStore};
use crate::skill_store::{SkillRevisionStore, SkillStorePaths};
use crate::storage::Storage;
use chrono::{Duration, Utc};
use serde_json::json;
use tempfile::{TempDir, tempdir};

struct SourceFixture {
    _app: TempDir,
    _cache: TempDir,
    state: SkillStateStore,
    paths: SkillStorePaths,
    store: SkillRevisionStore,
    source: ManagedSkillSource,
}

impl SourceFixture {
    async fn new() -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let state = SkillStateStore::new(storage);
        let store = SkillRevisionStore::new(paths.clone(), state.clone());
        let source = ManagedSkillSource::from_store(store.clone());
        Self {
            _app: app,
            _cache: cache,
            state,
            paths,
            store,
            source,
        }
    }

    async fn active(&self, id: &str) -> crate::skill_store::StoredSkillRevision {
        let package = write_package(id).await;
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
async fn discover_replaces_issues_before_early_root_error() {
    let fixture = SourceFixture::new().await;
    let managed = fixture.active("com.example.stale-issue").await;
    make_writable(&managed.path).await;
    tokio::fs::write(managed.path.join("SKILL.md"), "corrupt")
        .await
        .unwrap();
    fixture.source.discover().await.unwrap();
    assert_eq!(fixture.source.issues().len(), 1);
    tokio::fs::rename(
        &fixture.paths.managed,
        fixture.paths.managed.with_extension("missing"),
    )
    .await
    .unwrap();

    fixture.source.discover().await.unwrap_err();

    assert!(fixture.source.issues().is_empty());
}

#[tokio::test]
async fn managed_issue_has_discovery_timestamp() {
    let fixture = SourceFixture::new().await;
    let managed = fixture.active("com.example.timestamp").await;
    make_writable(&managed.path).await;
    tokio::fs::write(managed.path.join("SKILL.md"), "corrupt")
        .await
        .unwrap();
    let before = Utc::now() - Duration::seconds(1);

    fixture.source.discover().await.unwrap();

    let issue = fixture.source.issues().into_iter().next().unwrap();
    assert!(issue.recorded_at >= before);
    assert!(issue.recorded_at <= Utc::now() + Duration::seconds(1));
}

#[tokio::test]
async fn invalid_descriptor_quarantine_persists_actual_hash_and_parse_error() {
    let fixture = SourceFixture::new().await;
    let managed = fixture
        .active("com.example.invalid-descriptor-actual")
        .await;
    make_writable(&managed.path).await;
    tokio::fs::write(managed.path.join("agentweave.json"), "{invalid")
        .await
        .unwrap();
    let actual_hash = hash_package_tree(&managed.path).await.unwrap();

    assert!(fixture.source.discover().await.unwrap().is_empty());

    let record = fixture
        .state
        .get_revision(&managed.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.content_hash, actual_hash);
    assert_eq!(record.version, "1.0.0");
    assert_eq!(
        record.descriptor_json["id"],
        "com.example.invalid-descriptor-actual"
    );
    assert!(record.validation_json["descriptorError"].is_string());
    assert_eq!(record.validation_json["quarantined"], true);
}

#[tokio::test]
async fn quarantine_failure_is_audited_and_does_not_block_valid_peer() {
    let fixture = SourceFixture::new().await;
    let valid = fixture.active("com.example.valid-peer").await;
    let corrupt = fixture.active("com.example.missing-peer").await;
    make_writable(&corrupt.path).await;
    tokio::fs::remove_dir_all(&corrupt.path).await.unwrap();

    let discovered = fixture.source.discover().await.unwrap();

    assert_eq!(discovered.len(), 1);
    assert_eq!(
        discovered[0].descriptor.id.as_str(),
        "com.example.valid-peer"
    );
    assert_eq!(discovered[0].root, valid.path);
    let issue = fixture.source.issues().into_iter().next().unwrap();
    assert!(issue.quarantine_error.is_some());
    let audit = fixture
        .state
        .list_audit(&SkillPackageId::parse("com.example.missing-peer").unwrap())
        .await
        .unwrap();
    assert!(audit.iter().any(|entry| {
        entry.operation == "managed_discovery_quarantine_failed"
            && entry.revision_id.as_deref() == Some(corrupt.revision_id.as_str())
    }));
}

async fn write_package(id: &str) -> TempDir {
    let root = tempdir().unwrap();
    let name = id.rsplit('.').next().unwrap();
    tokio::fs::write(
        root.path().join("agentweave.json"),
        json!({
            "schemaVersion": 1,
            "id": id,
            "version": "1.0.0",
            "displayName": name,
            "kind": "instruction_only",
            "package": {
                "includeInstructions": true,
                "includeRuntime": false
            }
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("SKILL.md"),
        format!("---\nname: {name}\ndescription: {name}\n---\n{name}\n"),
    )
    .await
    .unwrap();
    root
}

async fn make_writable(path: &std::path::Path) {
    let mut entries = tokio::fs::read_dir(path).await.unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let metadata = entry.metadata().await.unwrap();
        let mut permissions = metadata.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            permissions.set_mode(permissions.mode() | 0o600);
        }
        #[cfg(not(unix))]
        permissions.set_readonly(false);
        tokio::fs::set_permissions(entry.path(), permissions)
            .await
            .unwrap();
    }
    let metadata = tokio::fs::metadata(path).await.unwrap();
    let mut permissions = metadata.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o700);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    tokio::fs::set_permissions(path, permissions).await.unwrap();
}
