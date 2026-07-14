use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_package::SkillPackageId;
use crate::skill_source::{
    ManagedSkillSource, SkillLayer, SkillSource, hash_package_tree, portable_collision_key,
    register_portable_path,
};
use crate::skill_state::{SkillLayerRecord, SkillRevisionStatus, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use semver::Version;
use serde_json::json;
use std::path::Path;
use std::sync::Arc;
use tempfile::{TempDir, tempdir};

struct StoreFixture {
    _app: TempDir,
    _cache: TempDir,
    storage: Storage,
    state: SkillStateStore,
    paths: SkillStorePaths,
    store: SkillRevisionStore,
}

impl StoreFixture {
    async fn new() -> Self {
        Self::with_limits_and_faults(
            SkillStoreLimits {
                max_file_bytes: 1024,
                max_package_bytes: 4096,
                ..SkillStoreLimits::default()
            },
            SkillStoreTestFaults::default(),
        )
        .await
    }

    async fn with_limits_and_faults(
        limits: SkillStoreLimits,
        faults: SkillStoreTestFaults,
    ) -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let state = SkillStateStore::new(storage.clone());
        let store =
            SkillRevisionStore::with_test_faults(paths.clone(), state.clone(), limits, faults);
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

#[tokio::test]
async fn authoritative_id_and_final_metadata_follow_staging_edits_into_managed() {
    let fixture = StoreFixture::new().await;
    let source = write_package("com.example.calendar", "Calendar v1").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let staged_record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(
        staged.path.file_name().unwrap(),
        staged.revision_id.as_str()
    );
    assert_eq!(staged_record.revision_id, staged.revision_id);
    assert_eq!(staged_record.storage_path, staged.path.to_string_lossy());
    assert_eq!(staged_record.status, SkillRevisionStatus::Staging);
    assert_eq!(staged_record.content_hash, staged.content_hash);

    fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("SKILL.md"),
            b"---\nname: calendar\ndescription: Calendar\n---\nCalendar v2\n",
        )
        .await
        .unwrap();
    let edited_record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(edited_record.content_hash, staged.content_hash);
    assert_eq!(edited_record.status, SkillRevisionStatus::Staging);
    let managed = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();
    let managed_record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();

    assert_ne!(managed.content_hash, staged.content_hash);
    assert_eq!(
        managed.path.file_name().unwrap(),
        staged.revision_id.as_str()
    );
    assert_eq!(managed_record.status, SkillRevisionStatus::Managed);
    assert_eq!(managed_record.storage_path, managed.path.to_string_lossy());
    assert_eq!(managed_record.content_hash, managed.content_hash);
    assert_eq!(managed_record.descriptor_json["id"], "com.example.calendar");
    assert_eq!(managed_record.validation_json["status"], "valid");
    assert!(!staged.path.exists());
    assert!(
        fixture
            .store
            .write_staging_file(&managed.revision_id, Path::new("changed.txt"), b"forbidden",)
            .await
            .unwrap_err()
            .to_string()
            .contains("editable staging")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn staging_rejects_symlinks_and_cleans_partial_state() {
    let fixture = StoreFixture::new().await;
    let source = write_package("com.example.calendar", "Calendar").await;
    std::os::unix::fs::symlink(
        source.path().join("SKILL.md"),
        source.path().join("linked.md"),
    )
    .unwrap();

    let error = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("symlink"));
    assert!(directory_is_empty(&fixture.paths.staging).await);
    assert_eq!(revision_count(&fixture).await, 0);
}

#[tokio::test]
async fn store_scanner_registry_rejects_portable_path_collisions() {
    let first = Path::new("nested/\u{3c3}.txt");
    let second = Path::new("nested/\u{3c2}.txt");
    let mut paths = std::collections::BTreeMap::new();
    register_portable_path(&mut paths, first, &portable_collision_key(first).unwrap()).unwrap();

    let error =
        register_portable_path(&mut paths, second, &portable_collision_key(second).unwrap())
            .unwrap_err();

    assert!(error.to_string().contains("portable path collision"));
}

#[tokio::test]
async fn staging_enforces_file_and_package_limits_without_leaving_state() {
    let file_fixture = StoreFixture::with_limits_and_faults(
        SkillStoreLimits {
            max_file_bytes: 300,
            max_package_bytes: 4096,
            ..SkillStoreLimits::default()
        },
        SkillStoreTestFaults::default(),
    )
    .await;
    let file_source = write_package("com.example.calendar", "Calendar").await;
    tokio::fs::write(file_source.path().join("large.bin"), vec![0_u8; 301])
        .await
        .unwrap();

    let file_error = file_fixture
        .store
        .create_staging_revision(file_source.path(), "owner-1")
        .await
        .unwrap_err();

    assert!(
        file_error
            .to_string()
            .contains("file exceeds 300 byte limit")
    );
    assert!(directory_is_empty(&file_fixture.paths.staging).await);
    assert_eq!(revision_count(&file_fixture).await, 0);

    let package_fixture = StoreFixture::with_limits_and_faults(
        SkillStoreLimits {
            max_file_bytes: 1024,
            max_package_bytes: 320,
            ..SkillStoreLimits::default()
        },
        SkillStoreTestFaults::default(),
    )
    .await;
    let package_source = write_package("com.example.calendar", "Calendar").await;
    tokio::fs::write(package_source.path().join("extra.bin"), vec![0_u8; 160])
        .await
        .unwrap();

    let package_error = package_fixture
        .store
        .create_staging_revision(package_source.path(), "owner-1")
        .await
        .unwrap_err();

    assert!(
        package_error
            .to_string()
            .contains("package exceeds 320 byte limit")
    );
    assert!(directory_is_empty(&package_fixture.paths.staging).await);
    assert_eq!(revision_count(&package_fixture).await, 0);
}

#[tokio::test]
async fn write_staging_file_rejects_absolute_parent_escape_and_package_overflow() {
    let fixture = StoreFixture::new().await;
    let source = write_package("com.example.calendar", "Calendar").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();

    assert!(
        fixture
            .store
            .write_staging_file(&staged.revision_id, Path::new("../escape"), b"x")
            .await
            .is_err()
    );
    assert!(
        fixture
            .store
            .write_staging_file(&staged.revision_id, Path::new("/absolute"), b"x")
            .await
            .is_err()
    );
    assert!(
        fixture
            .store
            .write_staging_file(
                &staged.revision_id,
                Path::new("large.bin"),
                &vec![0_u8; 4096],
            )
            .await
            .is_err()
    );
    assert!(!source.path().parent().unwrap().join("escape").exists());
}

#[tokio::test]
async fn staging_copy_failure_cleans_directory_and_database() {
    let faults = SkillStoreTestFaults::default();
    faults.fail_once(SkillStoreFaultPoint::StagingCopyFile);
    let fixture = StoreFixture::with_limits_and_faults(
        SkillStoreLimits {
            max_file_bytes: 1024,
            max_package_bytes: 4096,
            ..SkillStoreLimits::default()
        },
        faults,
    )
    .await;
    let source = write_package("com.example.calendar", "Calendar").await;

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
async fn promotion_copy_and_rename_failures_restore_staging_and_clean_incoming() {
    for point in [
        SkillStoreFaultPoint::IncomingCopyFile,
        SkillStoreFaultPoint::PromoteIncomingRename,
    ] {
        let faults = SkillStoreTestFaults::default();
        let fixture = StoreFixture::with_limits_and_faults(
            SkillStoreLimits {
                max_file_bytes: 1024,
                max_package_bytes: 4096,
                ..SkillStoreLimits::default()
            },
            faults.clone(),
        )
        .await;
        let source = write_package("com.example.calendar", "Calendar").await;
        let staged = fixture
            .store
            .create_staging_revision(source.path(), "owner-1")
            .await
            .unwrap();
        faults.fail_once(point);

        let error = fixture
            .store
            .promote_revision(&staged.revision_id)
            .await
            .unwrap_err();

        assert!(format!("{error:#}").contains(&format!("{point:?}")));
        assert!(staged.path.is_dir());
        assert!(directory_is_empty(&fixture.paths.managed.join(".incoming")).await);
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
async fn promotion_rejects_destination_collision_without_mutating_staging() {
    let fixture = StoreFixture::new().await;
    let source = write_package("com.example.calendar", "Calendar").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let destination = fixture
        .paths
        .managed
        .join("com.example.calendar/revisions")
        .join(&staged.revision_id);
    tokio::fs::create_dir_all(&destination).await.unwrap();

    let error = fixture
        .store
        .promote_revision(&staged.revision_id)
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

#[tokio::test]
async fn promotion_database_failure_restores_staging_and_removes_managed_copy() {
    let faults = SkillStoreTestFaults::default();
    let fixture = StoreFixture::with_limits_and_faults(
        SkillStoreLimits {
            max_file_bytes: 1024,
            max_package_bytes: 4096,
            ..SkillStoreLimits::default()
        },
        faults.clone(),
    )
    .await;
    let source = write_package("com.example.calendar", "Calendar").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    faults.fail_once(SkillStoreFaultPoint::PromoteDatabase);

    let error = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("PromoteDatabase"));
    assert!(staged.path.is_dir());
    assert!(!managed_path(&fixture, &staged.revision_id).exists());
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Staging);
    assert_eq!(record.storage_path, staged.path.to_string_lossy());
}

#[tokio::test]
async fn promotion_cleanup_failure_keeps_staging_authoritative_and_reports_both_errors() {
    let faults = SkillStoreTestFaults::default();
    let fixture = StoreFixture::with_limits_and_faults(
        SkillStoreLimits {
            max_file_bytes: 1024,
            max_package_bytes: 4096,
            ..SkillStoreLimits::default()
        },
        faults.clone(),
    )
    .await;
    let source = write_package("com.example.calendar", "Calendar").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    faults.fail_once(SkillStoreFaultPoint::PromoteDatabase);
    faults.fail_once(SkillStoreFaultPoint::PromoteRestoreRename);

    let error = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap_err();
    let message = format!("{error:#}");

    assert!(message.contains("PromoteDatabase"), "{message}");
    assert!(message.contains("PromoteRestoreRename"), "{message}");
    assert!(staged.path.exists());
    assert!(!managed_path(&fixture, &staged.revision_id).exists());
    assert!(!fixture.paths.quarantine.join(&staged.revision_id).exists());
    let maintenance = fixture.paths.quarantine.join(".maintenance");
    let mut entries = tokio::fs::read_dir(&maintenance).await.unwrap();
    assert!(entries.next_entry().await.unwrap().unwrap().path().is_dir());
    assert!(entries.next_entry().await.unwrap().is_none());
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Staging);
    assert_eq!(record.storage_path, staged.path.to_string_lossy());
    assert_eq!(fixture.store.maintenance_issues().len(), 1);
}

#[tokio::test]
async fn quarantine_moves_staging_and_managed_revisions_and_persists_reason() {
    for managed in [false, true] {
        let fixture = StoreFixture::new().await;
        let source = write_package("com.example.calendar", "Calendar").await;
        let staged = fixture
            .store
            .create_staging_revision(source.path(), "owner-1")
            .await
            .unwrap();
        let original = if managed {
            fixture
                .store
                .promote_revision(&staged.revision_id)
                .await
                .unwrap()
                .path
        } else {
            staged.path.clone()
        };

        let quarantined = fixture
            .store
            .quarantine_revision(&staged.revision_id, "validation_failed")
            .await
            .unwrap();
        let record = fixture
            .state
            .get_revision(&staged.revision_id)
            .await
            .unwrap()
            .unwrap();

        assert!(!original.exists());
        assert_eq!(
            quarantined.path,
            fixture.paths.quarantine.join(&staged.revision_id)
        );
        assert_eq!(record.status, SkillRevisionStatus::Quarantined);
        assert_eq!(
            record.validation_json["quarantineReason"],
            "validation_failed"
        );
    }
}

#[tokio::test]
async fn quarantine_database_failure_restores_original_path_and_lifecycle() {
    let faults = SkillStoreTestFaults::default();
    let fixture = StoreFixture::with_limits_and_faults(
        SkillStoreLimits {
            max_file_bytes: 1024,
            max_package_bytes: 4096,
            ..SkillStoreLimits::default()
        },
        faults.clone(),
    )
    .await;
    let source = write_package("com.example.calendar", "Calendar").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    faults.fail_once(SkillStoreFaultPoint::QuarantineDatabase);

    let error = fixture
        .store
        .quarantine_revision(&staged.revision_id, "broken")
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("QuarantineDatabase"));
    assert!(staged.path.is_dir());
    assert!(!fixture.paths.quarantine.join(&staged.revision_id).exists());
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
async fn quarantine_cleanup_failure_keeps_source_authoritative_and_reports_both_errors() {
    let faults = SkillStoreTestFaults::default();
    let fixture = StoreFixture::with_limits_and_faults(
        SkillStoreLimits {
            max_file_bytes: 1024,
            max_package_bytes: 4096,
            ..SkillStoreLimits::default()
        },
        faults.clone(),
    )
    .await;
    let source = write_package("com.example.calendar", "Calendar").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    faults.fail_once(SkillStoreFaultPoint::QuarantineDatabase);
    faults.fail_once(SkillStoreFaultPoint::QuarantineRestoreRename);

    let error = fixture
        .store
        .quarantine_revision(&staged.revision_id, "broken")
        .await
        .unwrap_err();
    let message = format!("{error:#}");

    assert!(message.contains("QuarantineDatabase"), "{message}");
    assert!(message.contains("QuarantineRestoreRename"), "{message}");
    assert!(staged.path.exists());
    assert!(fixture.paths.quarantine.join(&staged.revision_id).is_dir());
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
    assert_eq!(fixture.store.maintenance_issues().len(), 1);
}

#[tokio::test]
async fn managed_source_discovers_valid_revision_and_quarantines_corrupt_peer_with_issue() {
    let fixture = StoreFixture::new().await;
    let valid = stage_promote_activate(&fixture, "com.example.alpha").await;
    let corrupt = stage_promote_activate(&fixture, "com.example.beta").await;
    make_file_writable(&corrupt.path.join("SKILL.md")).await;
    tokio::fs::write(corrupt.path.join("SKILL.md"), "corrupt after promotion")
        .await
        .unwrap();
    let actual_hash = hash_package_tree(&corrupt.path).await.unwrap();
    let source = ManagedSkillSource::new(fixture.paths.clone(), fixture.state.clone());

    let discovered = source.discover().await.unwrap();
    let issues = source.issues();

    assert_eq!(source.layer(), SkillLayer::Managed);
    assert_eq!(discovered.len(), 1);
    assert_eq!(discovered[0].descriptor.id.as_str(), "com.example.alpha");
    assert_eq!(discovered[0].root, valid.path);
    assert_eq!(discovered[0].content_hash, valid.content_hash);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].revision_id, corrupt.revision_id);
    assert!(issues[0].reason.contains("content hash mismatch"));
    assert!(issues[0].quarantine_error.is_none());
    assert!(fixture.paths.quarantine.join(&corrupt.revision_id).is_dir());
    let record = fixture
        .state
        .get_revision(&corrupt.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Quarantined);
    assert_eq!(record.content_hash, actual_hash);
    assert_eq!(record.validation_json["status"], "invalid");
    assert!(
        record.validation_json["quarantineReason"]
            .as_str()
            .unwrap()
            .contains("content hash mismatch")
    );
}

#[tokio::test]
async fn managed_source_skips_descriptor_mismatch_path_escape_and_missing_path_with_issues() {
    let fixture = StoreFixture::new().await;
    let descriptor = stage_promote_activate(&fixture, "com.example.descriptor").await;
    make_file_writable(&descriptor.path.join("agentweave.json")).await;
    let mut descriptor_json: serde_json::Value = serde_json::from_slice(
        &tokio::fs::read(descriptor.path.join("agentweave.json"))
            .await
            .unwrap(),
    )
    .unwrap();
    descriptor_json["id"] = json!("com.example.changed");
    tokio::fs::write(
        descriptor.path.join("agentweave.json"),
        descriptor_json.to_string(),
    )
    .await
    .unwrap();

    let escaped = stage_promote_activate(&fixture, "com.example.escaped").await;
    let outside = tempdir().unwrap();
    sqlx::query("UPDATE skill_revisions SET storage_path = ? WHERE revision_id = ?")
        .bind(outside.path().to_string_lossy().as_ref())
        .bind(&escaped.revision_id)
        .execute(fixture.storage.pool())
        .await
        .unwrap();

    let missing = stage_promote_activate(&fixture, "com.example.missing").await;
    make_tree_writable_for_test(&missing.path).await;
    tokio::fs::remove_dir_all(&missing.path).await.unwrap();

    let source = ManagedSkillSource::new(fixture.paths.clone(), fixture.state.clone());
    let discovered = source.discover().await.unwrap();
    let issues = source.issues();

    assert!(discovered.is_empty());
    assert_eq!(issues.len(), 3);
    assert!(issues.iter().any(|issue| {
        issue.revision_id == descriptor.revision_id && issue.reason.contains("descriptor package")
    }));
    assert!(issues.iter().any(|issue| {
        issue.revision_id == escaped.revision_id
            && issue.reason.contains("storage path mismatch")
            && issue.quarantine_error.is_some()
    }));
    assert!(issues.iter().any(|issue| {
        issue.revision_id == missing.revision_id
            && issue.reason.contains("failed to inspect managed revision")
            && issue.quarantine_error.is_some()
    }));
    assert!(outside.path().is_dir());
}

#[tokio::test]
async fn quarantined_revision_is_not_returned_by_managed_source() {
    let fixture = StoreFixture::new().await;
    let managed = stage_promote_activate(&fixture, "com.example.calendar").await;
    fixture
        .store
        .quarantine_revision(&managed.revision_id, "revoked")
        .await
        .unwrap();
    let source = ManagedSkillSource::new(fixture.paths.clone(), fixture.state.clone());

    let discovered = source.discover().await.unwrap();

    assert!(discovered.is_empty());
    assert!(source.issues().is_empty());
}

#[tokio::test]
async fn managed_source_and_manager_reload_use_final_managed_path_hash_and_generation_guards() {
    let faults = SkillStoreTestFaults::default();
    let fixture = StoreFixture::with_limits_and_faults(
        SkillStoreLimits {
            max_file_bytes: 1024,
            max_package_bytes: 4096,
            ..SkillStoreLimits::default()
        },
        faults.clone(),
    )
    .await;
    let active = stage_promote_activate(&fixture, "com.example.calendar").await;
    let source = Arc::new(ManagedSkillSource::new(
        fixture.paths.clone(),
        fixture.state.clone(),
    ));
    let manager = SkillManager::new(manager_config(source)).await.unwrap();
    let previous = manager.current_snapshot();
    assert_eq!(previous.packages()[0].package.root, active.path);
    assert_eq!(
        previous.packages()[0].package.content_hash,
        active.content_hash
    );

    let package = write_package("com.example.calendar", "Calendar v2").await;
    let staged = fixture
        .store
        .create_staging_revision(package.path(), "owner-1")
        .await
        .unwrap();
    faults.fail_once(SkillStoreFaultPoint::PromoteDatabase);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let error = manager
        .reload_with_pre_publish(|_| async move {
            store.promote_revision(&revision_id).await.map(|_| ())
        })
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("PromoteDatabase"));
    assert!(Arc::ptr_eq(&previous, &manager.current_snapshot()));
    assert_eq!(manager.current_snapshot().generation(), 1);
}

#[tokio::test]
async fn staging_record_insert_failure_removes_copied_directory_and_database_row() {
    let fixture = StoreFixture::new().await;
    let source = write_package("com.example.calendar", "Calendar").await;
    sqlx::query(
        r#"CREATE TRIGGER fail_staging_record
           BEFORE INSERT ON skill_revisions
           BEGIN
             SELECT RAISE(ABORT, 'staging insert failed');
           END"#,
    )
    .execute(fixture.storage.pool())
    .await
    .unwrap();

    let error = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("staging insert failed"));
    assert!(directory_is_empty(&fixture.paths.staging).await);
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_revisions")
        .fetch_one(fixture.storage.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

async fn write_package(id: &str, body: &str) -> TempDir {
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
        format!("---\nname: {name}\ndescription: {name}\n---\n{body}\n"),
    )
    .await
    .unwrap();
    root
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

async fn revision_count(fixture: &StoreFixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM skill_revisions")
        .fetch_one(fixture.storage.pool())
        .await
        .unwrap()
}

fn managed_path(fixture: &StoreFixture, revision_id: &str) -> std::path::PathBuf {
    fixture
        .paths
        .managed
        .join("com.example.calendar/revisions")
        .join(revision_id)
}

async fn stage_promote_activate(
    fixture: &StoreFixture,
    id: &str,
) -> crate::skill_store::StoredSkillRevision {
    let package = write_package(id, id).await;
    let staged = fixture
        .store
        .create_staging_revision(package.path(), "owner-1")
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

fn manager_config(source: Arc<dyn SkillSource>) -> SkillManagerConfig {
    SkillManagerConfig {
        sources: vec![source],
        platform: PlatformId::Desktop,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: Version::new(0, 3, 0),
    }
}

async fn make_file_writable(path: &Path) {
    let metadata = tokio::fs::metadata(path).await.unwrap();
    let mut permissions = metadata.permissions();
    set_test_writable(&mut permissions, false);
    tokio::fs::set_permissions(path, permissions).await.unwrap();
}

async fn make_tree_writable_for_test(root: &Path) {
    let mut stack = vec![root.to_path_buf()];
    while let Some(path) = stack.pop() {
        let metadata = tokio::fs::metadata(&path).await.unwrap();
        let mut permissions = metadata.permissions();
        set_test_writable(&mut permissions, metadata.is_dir());
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

fn set_test_writable(permissions: &mut std::fs::Permissions, directory: bool) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(if directory { 0o755 } else { 0o644 });
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
}
