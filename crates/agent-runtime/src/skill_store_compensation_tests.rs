use crate::skill_state::{SkillRevisionStatus, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use serde_json::json;
use tempfile::{TempDir, tempdir};

struct CompensationFixture {
    _app: TempDir,
    _cache: TempDir,
    state: SkillStateStore,
    paths: SkillStorePaths,
    store: SkillRevisionStore,
    faults: SkillStoreTestFaults,
}

impl CompensationFixture {
    async fn new() -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let state = SkillStateStore::new(storage);
        let faults = SkillStoreTestFaults::default();
        let store = SkillRevisionStore::with_test_faults(
            paths.clone(),
            state.clone(),
            SkillStoreLimits {
                max_file_bytes: 2048,
                max_package_bytes: 8192,
                ..SkillStoreLimits::default()
            },
            faults.clone(),
        );
        Self {
            _app: app,
            _cache: cache,
            state,
            paths,
            store,
            faults,
        }
    }

    async fn stage(&self, id: &str) -> crate::skill_store::StoredSkillRevision {
        let source = write_package(id).await;
        self.store
            .create_staging_revision(source.path(), "owner-1")
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn promotion_db_and_destination_cleanup_failures_keep_staging_authoritative() {
    let fixture = CompensationFixture::new().await;
    let staged = fixture.stage("com.example.promote-cleanup").await;
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::PromoteDatabase);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::PromoteDestinationCleanup);

    let error = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("PromoteDatabase"), "{message}");
    assert!(message.contains("PromoteDestinationCleanup"), "{message}");
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Staging);
    assert_eq!(record.storage_path, staged.path.to_string_lossy());
    assert!(staged.path.is_dir());
    let canonical = fixture
        .paths
        .managed
        .join("com.example.promote-cleanup/revisions")
        .join(&staged.revision_id);
    assert!(!canonical.exists());
    let maintenance = fixture.paths.quarantine.join(".maintenance");
    let mut entries = tokio::fs::read_dir(&maintenance).await.unwrap();
    let isolated = entries.next_entry().await.unwrap().unwrap().path();
    assert!(isolated.is_dir());
    assert!(entries.next_entry().await.unwrap().is_none());
    assert!(
        fixture
            .store
            .maintenance_issues()
            .iter()
            .any(|issue| issue.revision_id == staged.revision_id)
    );

    let promoted = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();
    assert!(promoted.path.is_dir());
}

#[tokio::test]
async fn quarantine_db_and_destination_cleanup_failures_keep_source_authoritative() {
    let fixture = CompensationFixture::new().await;
    let staged = fixture.stage("com.example.quarantine-cleanup").await;
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::QuarantineDatabase);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::QuarantineDestinationCleanup);

    let error = fixture
        .store
        .quarantine_revision(&staged.revision_id, "invalid")
        .await
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("QuarantineDatabase"), "{message}");
    assert!(
        message.contains("QuarantineDestinationCleanup"),
        "{message}"
    );
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Staging);
    assert_eq!(record.storage_path, staged.path.to_string_lossy());
    assert!(staged.path.is_dir());
}

#[tokio::test]
async fn post_commit_promotion_cleanup_after_failure_returns_success_with_warning() {
    let fixture = CompensationFixture::new().await;
    let staged = fixture.stage("com.example.promote-warning").await;
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::PromoteSourceCleanupAfter);

    let managed = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();

    assert!(!staged.path.exists());
    assert!(managed.path.is_dir());
    assert_eq!(managed.maintenance_issues.len(), 1);
    assert!(
        managed.maintenance_issues[0]
            .message
            .contains("PromoteSourceCleanupAfter")
    );
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Managed);
    assert_eq!(record.storage_path, managed.path.to_string_lossy());
}

#[tokio::test]
async fn post_commit_quarantine_cleanup_failure_returns_success_with_warning() {
    let fixture = CompensationFixture::new().await;
    let staged = fixture.stage("com.example.quarantine-warning").await;
    let managed = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::QuarantineSourceCleanup);

    let quarantined = fixture
        .store
        .quarantine_revision(&staged.revision_id, "bad")
        .await
        .unwrap();

    assert!(managed.path.is_dir());
    assert!(quarantined.path.is_dir());
    assert_eq!(quarantined.maintenance_issues.len(), 1);
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(record.storage_path, quarantined.path.to_string_lossy());
}

#[tokio::test]
async fn after_operation_destination_cleanup_uncertainty_preserves_db_source_and_error_chain() {
    let fixture = CompensationFixture::new().await;
    let staged = fixture.stage("com.example.cleanup-after").await;
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::PromoteDatabase);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::PromoteDestinationCleanupAfter);

    let error = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("PromoteDatabase"), "{message}");
    assert!(
        message.contains("PromoteDestinationCleanupAfter"),
        "{message}"
    );
    assert!(staged.path.is_dir());
    let destination = fixture
        .paths
        .managed
        .join("com.example.cleanup-after/revisions")
        .join(&staged.revision_id);
    assert!(!destination.exists());
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.storage_path, staged.path.to_string_lossy());
}

#[tokio::test]
async fn managed_quarantine_collision_cleans_readonly_incoming_copy() {
    let fixture = CompensationFixture::new().await;
    let staged = fixture.stage("com.example.managed-collision").await;
    let managed = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();
    tokio::fs::create_dir(fixture.paths.quarantine.join(&managed.revision_id))
        .await
        .unwrap();

    fixture
        .store
        .quarantine_revision(&managed.revision_id, "collision")
        .await
        .unwrap_err();

    assert!(managed.path.is_dir());
    assert!(
        tokio::fs::read_dir(fixture.paths.quarantine.join(".incoming"))
            .await
            .unwrap()
            .next_entry()
            .await
            .unwrap()
            .is_none()
    );
}

async fn write_package(id: &str) -> TempDir {
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
