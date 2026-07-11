use crate::skill_state::{SkillRevisionStatus, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::skill_store_secure_fs::gate_secure_hash_after_open;
use crate::storage::Storage;
use serde_json::json;
use std::path::Path;
use tempfile::{TempDir, tempdir};

struct SecurityFixture {
    _app: TempDir,
    _cache: TempDir,
    storage: Storage,
    state: SkillStateStore,
    paths: SkillStorePaths,
    store: SkillRevisionStore,
    faults: SkillStoreTestFaults,
}

impl SecurityFixture {
    async fn new(limits: SkillStoreLimits) -> Self {
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
            limits,
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
}

fn small_limits() -> SkillStoreLimits {
    SkillStoreLimits {
        max_file_bytes: 2048,
        max_package_bytes: 8192,
        max_entries: 64,
        max_files: 32,
        max_directories: 16,
        max_depth: 8,
        max_relative_path_bytes: 256,
    }
}

#[tokio::test]
async fn package_tree_limits_reject_entry_file_directory_depth_and_path_count_bypasses() {
    let source = write_package("com.example.limits").await;
    tokio::fs::create_dir_all(source.path().join("nested/deeper"))
        .await
        .unwrap();
    tokio::fs::write(source.path().join("nested/deeper/zero"), [])
        .await
        .unwrap();
    tokio::fs::write(source.path().join("another-zero"), [])
        .await
        .unwrap();

    for (name, limits, expected) in [
        (
            "entries",
            SkillStoreLimits {
                max_entries: 2,
                ..small_limits()
            },
            "entry count",
        ),
        (
            "files",
            SkillStoreLimits {
                max_files: 2,
                ..small_limits()
            },
            "file count",
        ),
        (
            "directories",
            SkillStoreLimits {
                max_directories: 1,
                ..small_limits()
            },
            "directory count",
        ),
        (
            "depth",
            SkillStoreLimits {
                max_depth: 2,
                ..small_limits()
            },
            "depth",
        ),
        (
            "path bytes",
            SkillStoreLimits {
                max_relative_path_bytes: 8,
                ..small_limits()
            },
            "relative path",
        ),
    ] {
        let fixture = SecurityFixture::new(limits).await;
        let error = fixture
            .store
            .create_staging_revision(source.path(), "owner-1")
            .await
            .unwrap_err();
        assert!(error.to_string().contains(expected), "{name}: {error:#}");
        assert!(directory_is_empty(&fixture.paths.staging).await);
    }
}

#[tokio::test]
async fn staging_rejects_real_root_replacement_without_writing_replacement() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.real-root-swap").await;
    let old = fixture.paths.staging.with_extension("old");
    tokio::fs::rename(&fixture.paths.staging, &old)
        .await
        .unwrap();
    tokio::fs::create_dir(&fixture.paths.staging).await.unwrap();

    let error = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("identity"));
    assert!(directory_is_empty(&fixture.paths.staging).await);
}

#[tokio::test]
async fn staging_copy_rejects_reserved_revision_replacement_without_writing_it() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.reserved-revision-swap").await;
    let revision_id = SkillStateStore::allocate_revision_id();
    fixture.faults.set_revision_id_once(&revision_id);
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::CopyBeforeFileOpen);
    let store = fixture.store.clone();
    let source_path = source.path().to_path_buf();
    let staging =
        tokio::spawn(async move { store.create_staging_revision(&source_path, "owner-1").await });
    gate.wait_entered().await;
    let destination = fixture.paths.staging.join(&revision_id);
    let original = fixture
        .paths
        .staging
        .join(format!("{revision_id}.original"));
    tokio::fs::rename(&destination, &original).await.unwrap();
    tokio::fs::create_dir(&destination).await.unwrap();

    gate.release().await;
    let result = staging.await.unwrap();

    assert!(
        result.is_err(),
        "replacement revision must invalidate staging"
    );
    assert!(destination.is_dir());
    assert!(directory_is_empty(&destination).await);
}

#[cfg(unix)]
#[tokio::test]
async fn copy_open_rejects_file_swapped_to_symlink_after_scan() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.swap").await;
    let outside = tempdir().unwrap();
    let outside_file = outside.path().join("outside");
    tokio::fs::write(&outside_file, "outside").await.unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::CopyBeforeFileOpen);
    let store = fixture.store.clone();
    let source_path = source.path().to_path_buf();
    let stage =
        tokio::spawn(async move { store.create_staging_revision(&source_path, "owner-1").await });
    gate.wait_entered().await;
    tokio::fs::remove_file(source.path().join("SKILL.md"))
        .await
        .unwrap();
    std::os::unix::fs::symlink(&outside_file, source.path().join("SKILL.md")).unwrap();

    gate.release().await;
    let error = stage.await.unwrap().unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("symlink"), "{message}");
    assert_eq!(
        tokio::fs::read_to_string(outside_file).await.unwrap(),
        "outside"
    );
    assert!(directory_is_empty(&fixture.paths.staging).await);
}

#[cfg(unix)]
#[tokio::test]
async fn secure_hash_never_reads_outside_file_after_path_is_swapped_to_symlink() {
    let source = write_package("com.example.hash-swap").await;
    let outside = tempdir().unwrap();
    let outside_file = outside.path().join("outside");
    tokio::fs::write(&outside_file, "outside-secret")
        .await
        .unwrap();
    let gate = gate_secure_hash_after_open();
    let root = source.path().to_path_buf();
    let hashing = tokio::spawn(async move { crate::skill_source::hash_package_tree(&root).await });
    gate.wait_entered().await;
    tokio::fs::remove_file(source.path().join("SKILL.md"))
        .await
        .unwrap();
    std::os::unix::fs::symlink(&outside_file, source.path().join("SKILL.md")).unwrap();
    gate.release().await;

    let error = hashing.await.unwrap().unwrap_err();
    let message = format!("{error:#}");
    assert!(message.contains("symlink"), "{message}");
    assert_eq!(
        tokio::fs::read_to_string(outside_file).await.unwrap(),
        "outside-secret"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn staging_write_rejects_parent_swapped_to_symlink_before_temp_open() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.write-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    tokio::fs::create_dir(staged.path.join("nested"))
        .await
        .unwrap();
    let outside = tempdir().unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::WriteBeforeTempOpen);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let write = tokio::spawn(async move {
        store
            .write_staging_file(&revision_id, Path::new("nested/file"), b"secret")
            .await
    });
    gate.wait_entered().await;
    tokio::fs::rename(staged.path.join("nested"), staged.path.join("nested-old"))
        .await
        .unwrap();
    std::os::unix::fs::symlink(outside.path(), staged.path.join("nested")).unwrap();

    gate.release().await;
    let error = write.await.unwrap().unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("NotCommitted"), "{message}");
    assert!(!outside.path().join("file").exists());
}

#[tokio::test]
async fn staging_write_rejects_revision_replacement_before_temp_open() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.write-revision-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::WriteBeforeTempOpen);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let write = tokio::spawn(async move {
        store
            .write_staging_file(
                &revision_id,
                Path::new("SKILL.md"),
                b"---\nname: swapped\ndescription: changed\n---\nchanged\n",
            )
            .await
    });
    gate.wait_entered().await;
    let original = staged.path.with_extension("original");
    tokio::fs::rename(&staged.path, &original).await.unwrap();
    tokio::fs::create_dir(&staged.path).await.unwrap();
    for name in ["general-agent.json", "SKILL.md"] {
        tokio::fs::copy(original.join(name), staged.path.join(name))
            .await
            .unwrap();
    }
    let replacement_before = tokio::fs::read(staged.path.join("SKILL.md")).await.unwrap();

    gate.release().await;
    let result = write.await.unwrap();

    assert!(
        result.is_err(),
        "replacement revision must invalidate the write"
    );
    assert_eq!(
        tokio::fs::read(staged.path.join("SKILL.md")).await.unwrap(),
        replacement_before
    );
}

#[cfg(unix)]
#[tokio::test]
async fn staging_write_rejects_store_root_swapped_to_symlink() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.root-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let moved_root = fixture.paths.staging.with_extension("moved");
    tokio::fs::rename(&fixture.paths.staging, &moved_root)
        .await
        .unwrap();
    std::os::unix::fs::symlink(&moved_root, &fixture.paths.staging).unwrap();

    let error = fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("SKILL.md"),
            b"---\nname: root-swap\ndescription: bad\n---\nbad\n",
        )
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("store root must be a real directory"));
    let content = tokio::fs::read_to_string(moved_root.join(&staged.revision_id).join("SKILL.md"))
        .await
        .unwrap();
    assert!(!content.contains("description: bad"));
}

#[tokio::test]
async fn destination_empty_directory_created_after_check_is_not_replaced() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.reserve").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::PromoteBeforeDestinationCommit);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let promote = tokio::spawn(async move { store.promote_revision(&revision_id).await });
    gate.wait_entered().await;
    let destination = fixture
        .paths
        .managed
        .join("com.example.reserve/revisions")
        .join(&staged.revision_id);
    tokio::fs::create_dir(&destination).await.unwrap();

    gate.release().await;
    let error = promote.await.unwrap().unwrap_err();

    assert!(format!("{error:#}").contains("already exists"));
    assert!(destination.is_dir());
    assert!(directory_is_empty(&destination).await);
    assert!(staged.path.is_dir());
}

#[tokio::test]
async fn promotion_copy_rejects_incoming_revision_replacement_without_writing_it() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.incoming-revision-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::CopyBeforeFileOpen);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let promotion = tokio::spawn(async move { store.promote_revision(&revision_id).await });
    gate.wait_entered().await;
    let incoming_root = fixture.paths.managed.join(".incoming");
    let mut entries = tokio::fs::read_dir(&incoming_root).await.unwrap();
    let incoming = entries.next_entry().await.unwrap().unwrap().path();
    assert!(entries.next_entry().await.unwrap().is_none());
    let original = incoming.with_extension("original");
    tokio::fs::rename(&incoming, &original).await.unwrap();
    tokio::fs::create_dir(&incoming).await.unwrap();

    gate.release().await;
    let result = promotion.await.unwrap();

    assert!(
        result.is_err(),
        "replacement incoming revision must abort promotion"
    );
    assert!(incoming.is_dir());
    assert!(directory_is_empty(&incoming).await);
    assert!(staged.path.is_dir());
}

#[cfg(unix)]
#[tokio::test]
async fn promotion_readonly_rejects_destination_revision_replacement() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.readonly-revision-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::ManagedReadonlyBeforeApply);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let promotion = tokio::spawn(async move { store.promote_revision(&revision_id).await });
    gate.wait_entered().await;
    let destination = fixture
        .paths
        .managed
        .join("com.example.readonly-revision-swap/revisions")
        .join(&staged.revision_id);
    let original = destination.with_extension("original");
    tokio::fs::rename(&destination, &original).await.unwrap();
    tokio::fs::create_dir(&destination).await.unwrap();
    let marker = destination.join("replacement-marker");
    tokio::fs::write(&marker, b"replacement").await.unwrap();
    tokio::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600))
        .await
        .unwrap();

    gate.release().await;
    let result = promotion.await.unwrap();

    assert!(
        result.is_err(),
        "replacement destination must abort promotion"
    );
    assert_eq!(tokio::fs::read(&marker).await.unwrap(), b"replacement");
    assert_ne!(
        tokio::fs::metadata(&marker)
            .await
            .unwrap()
            .permissions()
            .mode()
            & 0o200,
        0
    );
}

#[tokio::test]
async fn promotion_cleanup_rejects_staging_revision_replacement() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.cleanup-revision-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::PromoteSourceCleanupBeforeApply);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let promotion = tokio::spawn(async move { store.promote_revision(&revision_id).await });
    gate.wait_entered().await;
    let original = staged.path.with_extension("cleanup-original");
    tokio::fs::rename(&staged.path, &original).await.unwrap();
    tokio::fs::create_dir(&staged.path).await.unwrap();
    let marker = staged.path.join("replacement-marker");
    tokio::fs::write(&marker, b"replacement").await.unwrap();

    gate.release().await;
    let promoted = promotion.await.unwrap().unwrap();

    assert!(promoted.path.is_dir());
    assert_eq!(tokio::fs::read(&marker).await.unwrap(), b"replacement");
    assert!(
        promoted
            .maintenance_issues
            .iter()
            .any(|issue| issue.operation == "promotion_source_cleanup")
    );
}

#[cfg(unix)]
#[tokio::test]
async fn promotion_rejects_managed_root_swap_without_writing_external_tree() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.managed-root-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::PromoteBeforeDestinationCommit);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let promotion = tokio::spawn(async move { store.promote_revision(&revision_id).await });
    gate.wait_entered().await;
    let moved = fixture.paths.managed.with_extension("moved");
    let outside = tempdir().unwrap();
    tokio::fs::rename(&fixture.paths.managed, &moved)
        .await
        .unwrap();
    std::os::unix::fs::symlink(outside.path(), &fixture.paths.managed).unwrap();
    gate.release().await;

    let error = promotion.await.unwrap().unwrap_err();

    assert!(format!("{error:#}").contains("identity"));
    assert!(staged.path.is_dir());
    assert!(directory_is_empty(outside.path()).await);
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
async fn promotion_rejects_real_managed_root_swap_during_destination_commit() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.real-managed-root-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::PromoteBeforeDestinationCommit);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let promotion = tokio::spawn(async move { store.promote_revision(&revision_id).await });
    gate.wait_entered().await;
    let moved = fixture.paths.managed.with_extension("moved-real");
    tokio::fs::rename(&fixture.paths.managed, &moved)
        .await
        .unwrap();
    tokio::fs::create_dir(&fixture.paths.managed).await.unwrap();
    gate.release().await;

    let error = promotion.await.unwrap().unwrap_err();

    assert!(format!("{error:#}").contains("identity"));
    assert!(directory_is_empty(&fixture.paths.managed).await);
    assert!(staged.path.is_dir());
}

#[cfg(unix)]
#[tokio::test]
async fn quarantine_rejects_root_swap_without_writing_external_tree() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.quarantine-root-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::QuarantineAfterLock);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let quarantine =
        tokio::spawn(async move { store.quarantine_revision(&revision_id, "root swap").await });
    gate.wait_entered().await;
    let moved = fixture.paths.quarantine.with_extension("moved");
    let outside = tempdir().unwrap();
    tokio::fs::rename(&fixture.paths.quarantine, &moved)
        .await
        .unwrap();
    std::os::unix::fs::symlink(outside.path(), &fixture.paths.quarantine).unwrap();
    gate.release().await;

    let error = quarantine.await.unwrap().unwrap_err();

    assert!(format!("{error:#}").contains("identity"));
    assert!(staged.path.is_dir());
    assert!(directory_is_empty(outside.path()).await);
}

#[tokio::test]
async fn quarantine_rejects_real_root_swap_during_operation() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.real-quarantine-root-swap").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::QuarantineAfterLock);
    let store = fixture.store.clone();
    let revision_id = staged.revision_id.clone();
    let quarantine =
        tokio::spawn(async move { store.quarantine_revision(&revision_id, "root swap").await });
    gate.wait_entered().await;
    let moved = fixture.paths.quarantine.with_extension("moved-real");
    tokio::fs::rename(&fixture.paths.quarantine, &moved)
        .await
        .unwrap();
    tokio::fs::create_dir(&fixture.paths.quarantine)
        .await
        .unwrap();
    gate.release().await;

    let error = quarantine.await.unwrap().unwrap_err();

    assert!(format!("{error:#}").contains("identity"));
    assert!(directory_is_empty(&fixture.paths.quarantine).await);
    assert!(staged.path.is_dir());
}

#[cfg(unix)]
#[tokio::test]
async fn promotion_preserves_executable_bits_and_clears_only_write_bits() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.mode").await;
    let script = source.path().join("run.sh");
    tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .mode(0o755)
        .open(&script)
        .await
        .unwrap();
    tokio::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .await
        .unwrap();
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

    let mode = tokio::fs::metadata(managed.path.join("run.sh"))
        .await
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o111, 0o111);
    assert_eq!(mode & 0o222, 0);
}

#[tokio::test]
async fn readonly_failure_rolls_back_before_database_promotion() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.permission").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::ManagedReadonly);

    let error = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("ManagedReadonly"));
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
async fn after_rename_failures_restore_original_tree_and_database_metadata() {
    for point in [
        SkillStoreFaultPoint::WriteAfterRenameMode,
        SkillStoreFaultPoint::WriteAfterRenameRevalidate,
    ] {
        let fixture = SecurityFixture::new(small_limits()).await;
        let source = write_package("com.example.after-rename").await;
        let staged = fixture
            .store
            .create_staging_revision(source.path(), "owner-1")
            .await
            .unwrap();
        let original = tokio::fs::read(staged.path.join("SKILL.md")).await.unwrap();
        fixture.faults.fail_once(point);

        let error = fixture
            .store
            .write_staging_file(
                &staged.revision_id,
                Path::new("SKILL.md"),
                b"---\nname: after-rename\ndescription: changed\n---\nchanged\n",
            )
            .await
            .unwrap_err();

        assert!(format!("{error:#}").contains(&format!("{point:?}")));
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
        assert_eq!(
            record.content_hash,
            crate::skill_source::hash_package_tree(&staged.path)
                .await
                .unwrap()
        );
    }
}

#[tokio::test]
async fn temp_cleanup_failure_is_reported_as_maintenance_issue_without_db_change() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.temp-cleanup").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let original = tokio::fs::read(staged.path.join("SKILL.md")).await.unwrap();
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteBeforeRename);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteTempCleanup);

    let error = fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("SKILL.md"),
            b"---\nname: temp-cleanup\ndescription: changed\n---\nchanged\n",
        )
        .await
        .unwrap_err();

    let message = format!("{error:#}");
    assert!(message.contains("WriteBeforeRename"), "{message}");
    assert!(message.contains("WriteTempCleanup"), "{message}");
    assert_eq!(
        tokio::fs::read(staged.path.join("SKILL.md")).await.unwrap(),
        original
    );
    let issues = fixture.store.maintenance_issues();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].operation, "staging_write_temp_cleanup");
    assert!(issues[0].path.exists());
}

#[tokio::test]
async fn failed_new_staging_write_removes_created_empty_parent_directories() {
    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.parents").await;
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    sqlx::query(
        r#"CREATE TRIGGER fail_nested_metadata
           BEFORE UPDATE OF content_hash ON skill_revisions
           BEGIN SELECT RAISE(ABORT, 'nested metadata failed'); END"#,
    )
    .execute(fixture.storage.pool())
    .await
    .unwrap();

    let error = fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("new/deep/file.txt"), b"new")
        .await
        .unwrap_err();

    assert!(format!("{error:#}").contains("nested metadata failed"));
    assert!(!staged.path.join("new").exists());
}

#[cfg(unix)]
#[tokio::test]
async fn readonly_source_is_copied_into_an_editable_staging_tree_without_losing_execute_bits() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = SecurityFixture::new(small_limits()).await;
    let source = write_package("com.example.readonly-source").await;
    let script = source.path().join("run.sh");
    tokio::fs::write(&script, "#!/bin/sh\n").await.unwrap();
    tokio::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o555))
        .await
        .unwrap();
    tokio::fs::set_permissions(
        source.path().join("SKILL.md"),
        std::fs::Permissions::from_mode(0o444),
    )
    .await
    .unwrap();
    tokio::fs::set_permissions(source.path(), std::fs::Permissions::from_mode(0o555))
        .await
        .unwrap();
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();

    fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("SKILL.md"),
            b"---\nname: readonly-source\ndescription: editable\n---\nedited\n",
        )
        .await
        .unwrap();

    let script_mode = tokio::fs::metadata(staged.path.join("run.sh"))
        .await
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(script_mode & 0o111, 0o111);
    assert_ne!(script_mode & 0o200, 0);
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

async fn directory_is_empty(path: &Path) -> bool {
    tokio::fs::read_dir(path)
        .await
        .unwrap()
        .next_entry()
        .await
        .unwrap()
        .is_none()
}
