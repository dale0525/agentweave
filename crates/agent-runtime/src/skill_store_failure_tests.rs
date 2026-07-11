use crate::skill_package::SkillPackageId;
use crate::skill_source::{ManagedSkillSource, SkillSource, hash_package_tree};
use crate::skill_state::{SkillLayerRecord, SkillRevisionStatus, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use serde_json::json;
use std::path::Path;
use tempfile::{TempDir, tempdir};

struct FailureFixture {
    _app: TempDir,
    _cache: TempDir,
    storage: Storage,
    state: SkillStateStore,
    paths: SkillStorePaths,
    store: SkillRevisionStore,
}

impl FailureFixture {
    async fn new(faults: SkillStoreTestFaults) -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let state = SkillStateStore::new(storage.clone());
        let store = SkillRevisionStore::with_test_faults(
            paths.clone(),
            state.clone(),
            SkillStoreLimits {
                max_file_bytes: 1024,
                max_package_bytes: 4096,
                ..SkillStoreLimits::default()
            },
            faults,
        );
        Self {
            _app: app,
            _cache: cache,
            storage,
            state,
            paths,
            store,
        }
    }
}

#[cfg(unix)]
#[tokio::test]
async fn store_path_preparation_rejects_symlinked_roots() {
    let app = tempdir().unwrap();
    let cache = tempdir().unwrap();
    let outside = tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), app.path().join("managed-skills")).unwrap();

    let error = SkillStorePaths::prepare(app.path(), cache.path())
        .await
        .unwrap_err();

    assert!(error.to_string().contains("must be a real directory"));
}

#[tokio::test]
async fn partial_staging_copy_failure_removes_partial_directory() {
    let faults = SkillStoreTestFaults::default();
    faults.fail_after(SkillStoreFaultPoint::StagingCopyFile, 1);
    let fixture = FailureFixture::new(faults).await;
    let source = write_package("com.example.partial").await;

    let error = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("StagingCopyFile"));
    assert!(directory_is_empty(&fixture.paths.staging).await);
    assert_eq!(revision_count(&fixture).await, 0);
}

#[tokio::test]
async fn staging_metadata_database_failure_restores_file_and_hash() {
    let fixture = FailureFixture::new(SkillStoreTestFaults::default()).await;
    let source = write_package("com.example.edit").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let original = tokio::fs::read(staged.path.join("SKILL.md")).await.unwrap();
    sqlx::query(
        r#"CREATE TRIGGER fail_staging_metadata
           BEFORE UPDATE OF content_hash ON skill_revisions
           WHEN OLD.lifecycle_status = 'staging'
           BEGIN
             SELECT RAISE(ABORT, 'metadata refresh failed');
           END"#,
    )
    .execute(fixture.storage.pool())
    .await
    .unwrap();

    let error = fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), b"changed")
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("metadata refresh failed"));
    assert_eq!(
        tokio::fs::read(staged.path.join("SKILL.md")).await.unwrap(),
        original
    );
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.content_hash, staged.content_hash);
    assert_eq!(
        hash_package_tree(&staged.path).await.unwrap(),
        staged.content_hash
    );
}

#[tokio::test]
async fn sqlite_promotion_failure_restores_staging_without_managed_or_incoming_content() {
    let fixture = FailureFixture::new(SkillStoreTestFaults::default()).await;
    let source = write_package("com.example.promote").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    sqlx::query(
        r#"CREATE TRIGGER fail_promotion_transition
           BEFORE UPDATE OF lifecycle_status ON skill_revisions
           WHEN NEW.lifecycle_status = 'managed'
           BEGIN
             SELECT RAISE(ABORT, 'promotion transition failed');
           END"#,
    )
    .execute(fixture.storage.pool())
    .await
    .unwrap();

    let error = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("promotion transition failed"));
    assert!(staged.path.is_dir());
    assert!(!managed_path(&fixture, "com.example.promote", &staged.revision_id).exists());
    assert!(
        !fixture
            .paths
            .managed
            .join(".incoming")
            .join(&staged.revision_id)
            .exists()
    );
    assert_eq!(
        fixture
            .state
            .get_revision(&staged.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Staging
    );
}

#[tokio::test]
async fn promotion_staging_rename_failure_cleans_incoming_and_preserves_staging() {
    let faults = SkillStoreTestFaults::default();
    let fixture = FailureFixture::new(faults.clone()).await;
    let source = write_package("com.example.rename").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    faults.fail_once(SkillStoreFaultPoint::PromoteStagingRename);

    let error = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("PromoteStagingRename"));
    assert!(staged.path.is_dir());
    assert!(
        !fixture
            .paths
            .managed
            .join(".incoming")
            .join(&staged.revision_id)
            .exists()
    );
    assert_eq!(
        fixture
            .state
            .get_revision(&staged.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Staging
    );
}

#[tokio::test]
async fn persistent_promotion_and_cleanup_failures_leave_staging_authoritative() {
    let faults = SkillStoreTestFaults::default();
    let fixture = FailureFixture::new(faults.clone()).await;
    let source = write_package("com.example.fallback").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    sqlx::query(
        r#"CREATE TRIGGER fail_all_lifecycle_updates
           BEFORE UPDATE OF lifecycle_status ON skill_revisions
           WHEN NEW.lifecycle_status != OLD.lifecycle_status
           BEGIN
             SELECT RAISE(ABORT, 'all lifecycle transitions failed');
           END"#,
    )
    .execute(fixture.storage.pool())
    .await
    .unwrap();
    faults.fail_once(SkillStoreFaultPoint::PromoteRestoreRename);

    let error = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap_err();
    let message = format!("{error:#}");

    assert!(
        message.contains("all lifecycle transitions failed"),
        "{message}"
    );
    assert!(message.contains("PromoteRestoreRename"), "{message}");
    assert!(staged.path.is_dir());
    assert!(!fixture.paths.quarantine.join(&staged.revision_id).exists());
    assert!(managed_path(&fixture, "com.example.fallback", &staged.revision_id).exists());
    assert_eq!(fixture.store.maintenance_issues().len(), 1);
    assert_eq!(
        fixture
            .state
            .get_revision(&staged.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Staging
    );
}

#[tokio::test]
async fn sqlite_quarantine_failure_restores_readonly_managed_revision() {
    let fixture = FailureFixture::new(SkillStoreTestFaults::default()).await;
    let managed = promote_activate(&fixture, "com.example.quarantine").await;
    sqlx::query(
        r#"CREATE TRIGGER fail_quarantine_transition
           BEFORE UPDATE OF lifecycle_status ON skill_revisions
           WHEN NEW.lifecycle_status = 'quarantined'
           BEGIN
             SELECT RAISE(ABORT, 'quarantine transition failed');
           END"#,
    )
    .execute(fixture.storage.pool())
    .await
    .unwrap();

    let error = fixture
        .store
        .quarantine_revision(&managed.revision_id, "broken")
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("quarantine transition failed"));
    assert!(managed.path.is_dir());
    assert!(!fixture.paths.quarantine.join(&managed.revision_id).exists());
    let record = fixture
        .state
        .get_revision(&managed.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Managed);
    assert_eq!(record.storage_path, managed.path.to_string_lossy());
}

#[tokio::test]
async fn quarantine_rename_failures_clean_temporary_content_and_preserve_source() {
    for point in [
        SkillStoreFaultPoint::QuarantineIncomingRename,
        SkillStoreFaultPoint::QuarantineSourceRename,
    ] {
        let faults = SkillStoreTestFaults::default();
        let fixture = FailureFixture::new(faults.clone()).await;
        let source = write_package("com.example.rename").await;
        let staged = fixture
            .store
            .create_staging_revision(source.path(), "owner-1")
            .await
            .unwrap();
        faults.fail_once(point);

        let error = fixture
            .store
            .quarantine_revision(&staged.revision_id, "broken")
            .await
            .unwrap_err();

        assert!(format!("{error:#}").contains(&format!("{point:?}")));
        assert!(staged.path.is_dir());
        assert!(!fixture.paths.quarantine.join(&staged.revision_id).exists());
        assert!(directory_is_empty(&fixture.paths.quarantine.join(".incoming")).await);
        assert_eq!(
            fixture
                .state
                .get_revision(&staged.revision_id)
                .await
                .unwrap()
                .unwrap()
                .status,
            SkillRevisionStatus::Staging
        );
    }
}

#[tokio::test]
async fn persistent_quarantine_and_cleanup_failures_leave_managed_authoritative() {
    let faults = SkillStoreTestFaults::default();
    let fixture = FailureFixture::new(faults.clone()).await;
    let managed = promote_activate(&fixture, "com.example.fallback").await;
    sqlx::query(
        r#"CREATE TRIGGER fail_all_quarantine_updates
           BEFORE UPDATE OF lifecycle_status ON skill_revisions
           WHEN NEW.lifecycle_status = 'quarantined'
           BEGIN
             SELECT RAISE(ABORT, 'all quarantine transitions failed');
           END"#,
    )
    .execute(fixture.storage.pool())
    .await
    .unwrap();
    faults.fail_once(SkillStoreFaultPoint::QuarantineRestoreRename);

    let error = fixture
        .store
        .quarantine_revision(&managed.revision_id, "broken")
        .await
        .unwrap_err();
    let message = format!("{error:#}");

    assert!(
        message.contains("all quarantine transitions failed"),
        "{message}"
    );
    assert!(message.contains("QuarantineRestoreRename"), "{message}");
    assert!(managed.path.is_dir());
    assert!(fixture.paths.quarantine.join(&managed.revision_id).exists());
    assert_eq!(fixture.store.maintenance_issues().len(), 1);
    assert_eq!(
        fixture
            .state
            .get_revision(&managed.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Managed
    );
}

#[tokio::test]
async fn quarantine_destination_collision_preserves_original_revision() {
    let fixture = FailureFixture::new(SkillStoreTestFaults::default()).await;
    let source = write_package("com.example.collision").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    tokio::fs::create_dir(fixture.paths.quarantine.join(&staged.revision_id))
        .await
        .unwrap();

    let error = fixture
        .store
        .quarantine_revision(&staged.revision_id, "broken")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("already exists"));
    assert!(staged.path.is_dir());
    assert_eq!(
        fixture
            .state
            .get_revision(&staged.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Staging
    );
}

#[cfg(unix)]
#[tokio::test]
async fn staging_rejects_non_regular_socket_entry_without_state() {
    let fixture = FailureFixture::new(SkillStoreTestFaults::default()).await;
    let source = write_package("com.example.socket").await;
    let _socket = std::os::unix::net::UnixListener::bind(source.path().join("socket")).unwrap();

    let error = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("special files"));
    assert!(directory_is_empty(&fixture.paths.staging).await);
    assert_eq!(revision_count(&fixture).await, 0);
}

#[tokio::test]
async fn managed_source_orders_active_packages_deterministically() {
    let fixture = FailureFixture::new(SkillStoreTestFaults::default()).await;
    promote_activate(&fixture, "com.example.zeta").await;
    promote_activate(&fixture, "com.example.alpha").await;
    let source = ManagedSkillSource::new(fixture.paths.clone(), fixture.state.clone());

    let packages = source.discover().await.unwrap();

    assert_eq!(packages.len(), 2);
    assert_eq!(packages[0].descriptor.id.as_str(), "com.example.alpha");
    assert_eq!(packages[1].descriptor.id.as_str(), "com.example.zeta");
}

#[cfg(all(unix, not(target_os = "macos")))]
#[tokio::test]
async fn non_utf8_managed_root_rejects_promotion_before_filesystem_mutation() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let root = tempdir().unwrap();
    let app = root.path().join(OsString::from_vec(b"app-\xff".to_vec()));
    let cache = tempdir().unwrap();
    let paths = SkillStorePaths::prepare(&app, cache.path()).await.unwrap();
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let store = SkillRevisionStore::new(paths.clone(), state.clone());
    let source = write_package("com.example.nonutf").await;
    let staged = store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();

    let error = store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap_err();

    assert!(error.to_string().contains("must be UTF-8"));
    assert!(staged.path.is_dir());
    assert_eq!(
        state
            .get_revision(&staged.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Staging
    );
    assert!(
        !paths
            .managed
            .join("com.example.nonutf/revisions")
            .join(&staged.revision_id)
            .exists()
    );
}

async fn promote_activate(
    fixture: &FailureFixture,
    id: &str,
) -> crate::skill_store::StoredSkillRevision {
    let source = write_package(id).await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let managed = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();
    fixture
        .state
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

async fn revision_count(fixture: &FailureFixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM skill_revisions")
        .fetch_one(fixture.storage.pool())
        .await
        .unwrap()
}

async fn directory_is_empty(path: &Path) -> bool {
    tokio::fs::read_dir(path)
        .await
        .unwrap()
        .next_entry()
        .await
        .unwrap()
        .is_none()
}

fn managed_path(
    fixture: &FailureFixture,
    package_id: &str,
    revision_id: &str,
) -> std::path::PathBuf {
    fixture
        .paths
        .managed
        .join(package_id)
        .join("revisions")
        .join(revision_id)
}
