use crate::skill_source::hash_package_tree;
use crate::skill_state::{SkillRevisionStatus, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use serde_json::json;
use std::path::Path;
use tempfile::{TempDir, tempdir};

struct RecoveryFixture {
    _app: TempDir,
    _cache: TempDir,
    storage: Storage,
    state: SkillStateStore,
    paths: SkillStorePaths,
    store: SkillRevisionStore,
    faults: SkillStoreTestFaults,
}

impl RecoveryFixture {
    async fn new() -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let state = SkillStateStore::new(storage.clone());
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
            storage,
            state,
            paths,
            store,
            faults,
        }
    }

    async fn staged(&self) -> crate::skill_store::StoredSkillRevision {
        let source = write_package().await;
        self.store
            .create_staging_revision(source.path(), "owner-1")
            .await
            .unwrap()
    }

    async fn fail_staging_metadata(&self) {
        sqlx::query(
            r#"CREATE TRIGGER fail_staging_metadata_for_recovery
               BEFORE UPDATE OF content_hash ON skill_revisions
               WHEN NEW.lifecycle_status = 'staging'
               BEGIN SELECT RAISE(ABORT, 'metadata recovery test failure'); END"#,
        )
        .execute(self.storage.pool())
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn metadata_and_restore_failure_quarantines_actual_tree_with_final_hash() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    fixture.fail_staging_metadata().await;
    fixture.faults.fail_once(SkillStoreFaultPoint::WriteRestore);

    let error = fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), edited_skill())
        .await
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(
        message.contains("metadata recovery test failure"),
        "{message}"
    );
    assert!(message.contains("WriteRestore"), "{message}");
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert!(Path::new(&record.storage_path).is_dir());
    assert_eq!(
        record.content_hash,
        hash_package_tree(Path::new(&record.storage_path))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn committed_write_and_restore_failure_quarantines_actual_tree() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteAfterRenameMode);
    fixture
        .faults
        .fail_after(SkillStoreFaultPoint::WriteBeforeRename, 1);

    let error = fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), edited_skill())
        .await
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("WriteAfterRenameMode"), "{message}");
    assert!(message.contains("WriteBeforeRename"), "{message}");
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(
        record.content_hash,
        hash_package_tree(Path::new(&record.storage_path))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn committed_invalid_descriptor_and_restore_failure_isolated_with_actual_hash() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteAfterRenameRevalidate);
    fixture
        .faults
        .fail_after(SkillStoreFaultPoint::WriteBeforeRename, 1);

    fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("general-agent.json"),
            b"{invalid",
        )
        .await
        .unwrap_err();

    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(
        record.content_hash,
        hash_package_tree(Path::new(&record.storage_path))
            .await
            .unwrap()
    );
    assert!(record.validation_json["descriptorError"].is_string());
}

#[tokio::test]
async fn committed_write_isolation_db_and_second_restore_failure_retries_quarantine_cas() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteAfterRenameMode);
    fixture
        .faults
        .fail_after(SkillStoreFaultPoint::WriteBeforeRename, 1);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteIsolationDatabase);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteIsolationRestore);

    let error = fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), edited_skill())
        .await
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("WriteIsolationDatabase"), "{message}");
    assert!(message.contains("WriteIsolationRestore"), "{message}");
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(
        record.content_hash,
        hash_package_tree(Path::new(&record.storage_path))
            .await
            .unwrap()
    );
    assert!(Path::new(&record.storage_path).is_dir());
}

#[tokio::test]
async fn isolation_collision_keeps_staging_path_authoritative_and_quarantined() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    fixture.fail_staging_metadata().await;
    fixture.faults.fail_once(SkillStoreFaultPoint::WriteRestore);
    let collision = fixture.paths.quarantine.join(&staged.revision_id);
    tokio::fs::create_dir(&collision).await.unwrap();
    tokio::fs::write(collision.join("external"), "keep")
        .await
        .unwrap();

    fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), edited_skill())
        .await
        .unwrap_err();

    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(record.storage_path, staged.path.to_string_lossy());
    assert!(staged.path.is_dir());
    assert_eq!(
        tokio::fs::read_to_string(collision.join("external"))
            .await
            .unwrap(),
        "keep"
    );
    assert_eq!(
        record.content_hash,
        hash_package_tree(&staged.path).await.unwrap()
    );
}

#[tokio::test]
async fn isolation_copy_failure_keeps_existing_authoritative_path() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    fixture.fail_staging_metadata().await;
    fixture.faults.fail_once(SkillStoreFaultPoint::WriteRestore);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteIsolationCopy);

    fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), edited_skill())
        .await
        .unwrap_err();

    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(record.storage_path, staged.path.to_string_lossy());
    assert!(Path::new(&record.storage_path).is_dir());
}

#[tokio::test]
async fn isolation_database_failure_leaves_db_path_existing_and_combines_errors() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    fixture.fail_staging_metadata().await;
    fixture.faults.fail_once(SkillStoreFaultPoint::WriteRestore);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteIsolationDatabase);

    let error = fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), edited_skill())
        .await
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("WriteRestore"), "{message}");
    assert!(message.contains("WriteIsolationDatabase"), "{message}");
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Staging);
    assert!(Path::new(&record.storage_path).is_dir());
    assert_eq!(
        record.content_hash,
        hash_package_tree(Path::new(&record.storage_path))
            .await
            .unwrap()
    );
    assert_eq!(
        tokio::fs::read_to_string(Path::new(&record.storage_path).join("SKILL.md"))
            .await
            .unwrap(),
        std::str::from_utf8(initial_skill()).unwrap()
    );
}

#[tokio::test]
async fn invalid_descriptor_and_restore_failure_quarantines_raw_tree_with_prior_descriptor() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    let prior = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    fixture.faults.fail_once(SkillStoreFaultPoint::WriteRestore);

    let error = fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("general-agent.json"),
            b"{invalid",
        )
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("WriteRestore"));
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(record.version, prior.version);
    assert_eq!(record.descriptor_json, prior.descriptor_json);
    assert!(record.validation_json.get("descriptorError").is_some());
    assert!(Path::new(&record.storage_path).is_dir());
    assert_eq!(
        record.content_hash,
        hash_package_tree(Path::new(&record.storage_path))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn invalid_descriptor_isolation_collision_keeps_actual_staging_tree_quarantined() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    fixture.faults.fail_once(SkillStoreFaultPoint::WriteRestore);
    let collision = fixture.paths.quarantine.join(&staged.revision_id);
    tokio::fs::create_dir(&collision).await.unwrap();
    tokio::fs::write(collision.join("external"), "keep")
        .await
        .unwrap();

    fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("general-agent.json"),
            b"{invalid",
        )
        .await
        .unwrap_err();

    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(record.storage_path, staged.path.to_string_lossy());
    assert!(record.validation_json.get("descriptorError").is_some());
    assert_eq!(
        tokio::fs::read_to_string(collision.join("external"))
            .await
            .unwrap(),
        "keep"
    );
}

#[tokio::test]
async fn invalid_descriptor_isolation_db_failure_keeps_db_path_existing() {
    let fixture = RecoveryFixture::new().await;
    let staged = fixture.staged().await;
    fixture.faults.fail_once(SkillStoreFaultPoint::WriteRestore);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteIsolationDatabase);

    let error = fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("general-agent.json"),
            b"{invalid",
        )
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("WriteIsolationDatabase"));
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Staging);
    assert!(Path::new(&record.storage_path).is_dir());
    assert_eq!(
        record.content_hash,
        hash_package_tree(Path::new(&record.storage_path))
            .await
            .unwrap()
    );
    let descriptor: serde_json::Value = serde_json::from_slice(
        &tokio::fs::read(Path::new(&record.storage_path).join("general-agent.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(descriptor["id"], record.descriptor_json["id"]);
}

fn edited_skill() -> &'static [u8] {
    b"---\nname: recovery\ndescription: edited\n---\nedited\n"
}

fn initial_skill() -> &'static [u8] {
    b"---\nname: recovery\ndescription: initial\n---\ninitial\n"
}

async fn write_package() -> TempDir {
    let root = tempdir().unwrap();
    tokio::fs::write(
        root.path().join("general-agent.json"),
        json!({
            "schemaVersion": 1,
            "id": "com.example.recovery",
            "version": "1.0.0",
            "displayName": "recovery",
            "kind": "instruction_only",
            "package": {"includeInstructions": true, "includeRuntime": false}
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(root.path().join("SKILL.md"), initial_skill())
        .await
        .unwrap();
    root
}
