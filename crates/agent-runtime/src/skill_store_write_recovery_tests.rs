use crate::skill_source::hash_package_tree;
use crate::skill_state::{SkillRevisionRecord, SkillRevisionStatus, SkillStateStore};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use serde_json::json;
use std::path::Path;
use tempfile::{TempDir, tempdir};

struct CowFixture {
    _app: TempDir,
    _cache: TempDir,
    storage: Storage,
    state: SkillStateStore,
    store: SkillRevisionStore,
    faults: SkillStoreTestFaults,
}

impl CowFixture {
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
            paths,
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

    async fn record(&self, revision_id: &str) -> SkillRevisionRecord {
        self.state.get_revision(revision_id).await.unwrap().unwrap()
    }
}

#[tokio::test]
async fn persistent_database_and_restore_failures_preserve_authoritative_tree_binding() {
    let fixture = CowFixture::new().await;
    let staged = fixture.staged().await;
    let prior = fixture.record(&staged.revision_id).await;
    sqlx::query(
        "CREATE TRIGGER reject_task7_revision_updates \
         BEFORE UPDATE ON skill_revisions \
         BEGIN SELECT RAISE(ABORT, 'persistent task7 database failure'); END",
    )
    .execute(fixture.storage.pool())
    .await
    .unwrap();
    fixture.faults.fail_once(SkillStoreFaultPoint::WriteRestore);
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteIsolationRestore);

    fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), edited_skill())
        .await
        .unwrap_err();

    assert_authoritative_binding(&fixture.record(&staged.revision_id).await, &prior).await;
    assert_eq!(
        tokio::fs::read(Path::new(&prior.storage_path).join("SKILL.md"))
            .await
            .unwrap(),
        initial_skill()
    );
}

#[tokio::test]
async fn committed_candidate_write_failure_never_changes_authoritative_tree() {
    let fixture = CowFixture::new().await;
    let staged = fixture.staged().await;
    let prior = fixture.record(&staged.revision_id).await;
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteAfterRenameMode);

    fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), edited_skill())
        .await
        .unwrap_err();

    assert_authoritative_binding(&fixture.record(&staged.revision_id).await, &prior).await;
}

#[tokio::test]
async fn invalid_candidate_descriptor_never_changes_authoritative_tree() {
    let fixture = CowFixture::new().await;
    let staged = fixture.staged().await;
    let prior = fixture.record(&staged.revision_id).await;

    fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("agentweave.json"),
            b"{invalid",
        )
        .await
        .unwrap_err();

    assert_authoritative_binding(&fixture.record(&staged.revision_id).await, &prior).await;
}

#[tokio::test]
async fn child_parent_failure_removes_candidate_and_preserves_authoritative_tree() {
    let fixture = CowFixture::new().await;
    let source = write_package().await;
    tokio::fs::write(source.path().join("a"), b"authoritative")
        .await
        .unwrap();
    let staged = fixture
        .store
        .create_staging_revision(source.path(), "owner-1")
        .await
        .unwrap();
    let prior = fixture.record(&staged.revision_id).await;

    fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("a/b"), b"rejected")
        .await
        .unwrap_err();

    assert_authoritative_binding(&fixture.record(&staged.revision_id).await, &prior).await;
    assert_eq!(
        tokio::fs::read(Path::new(&prior.storage_path).join("a"))
            .await
            .unwrap(),
        b"authoritative"
    );
    let candidate_prefix = format!("{}.candidate.", staged.revision_id);
    let mut entries = tokio::fs::read_dir(staged.path.parent().unwrap())
        .await
        .unwrap();
    while let Some(entry) = entries.next_entry().await.unwrap() {
        assert!(
            !entry
                .file_name()
                .to_string_lossy()
                .starts_with(&candidate_prefix),
            "staging candidate residue remained: {}",
            entry.path().display()
        );
    }
}

#[tokio::test]
async fn candidate_cleanup_failure_records_issue_without_hiding_write_error() {
    let fixture = CowFixture::new().await;
    let staged = fixture.staged().await;
    fixture
        .faults
        .fail_once(SkillStoreFaultPoint::WriteCandidateCleanup);

    let error = fixture
        .store
        .write_staging_file(
            &staged.revision_id,
            Path::new("agentweave.json"),
            b"{invalid",
        )
        .await
        .unwrap_err();

    let primary_message = error.root_cause().to_string();
    let message = format!("{error:#}");
    let primary = message.find(&primary_message).unwrap();
    let cleanup = message.find("WriteCandidateCleanup").unwrap();
    assert!(primary < cleanup, "primary error was hidden: {message}");
    let issues = fixture.store.maintenance_issues();
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].operation, "staging_candidate_cleanup");
    assert!(issues[0].message.contains("WriteCandidateCleanup"));
}

#[tokio::test]
async fn successful_write_switches_database_to_existing_candidate_before_old_cleanup() {
    let fixture = CowFixture::new().await;
    let staged = fixture.staged().await;

    fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("SKILL.md"), edited_skill())
        .await
        .unwrap();

    let record = fixture.record(&staged.revision_id).await;
    assert_eq!(record.status, SkillRevisionStatus::Staging);
    assert_ne!(record.storage_path, staged.path.to_string_lossy());
    assert!(
        Path::new(&record.storage_path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(&format!("{}.candidate.", staged.revision_id))
    );
    assert!(!staged.path.exists());
    assert_eq!(
        record.content_hash,
        hash_package_tree(Path::new(&record.storage_path))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn consecutive_writes_and_promotion_follow_authoritative_storage_path() {
    let fixture = CowFixture::new().await;
    let staged = fixture.staged().await;
    fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("first.txt"), b"first")
        .await
        .unwrap();
    fixture
        .store
        .write_staging_file(&staged.revision_id, Path::new("second.txt"), b"second")
        .await
        .unwrap();

    let managed = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();

    assert_eq!(
        tokio::fs::read(managed.path.join("first.txt"))
            .await
            .unwrap(),
        b"first"
    );
    assert_eq!(
        tokio::fs::read(managed.path.join("second.txt"))
            .await
            .unwrap(),
        b"second"
    );
}

async fn assert_authoritative_binding(actual: &SkillRevisionRecord, prior: &SkillRevisionRecord) {
    assert_eq!(actual.status, prior.status);
    assert_eq!(actual.storage_path, prior.storage_path);
    assert_eq!(actual.content_hash, prior.content_hash);
    assert_eq!(actual.descriptor_json, prior.descriptor_json);
    assert_eq!(
        actual.content_hash,
        hash_package_tree(Path::new(&actual.storage_path))
            .await
            .unwrap()
    );
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
        root.path().join("agentweave.json"),
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
