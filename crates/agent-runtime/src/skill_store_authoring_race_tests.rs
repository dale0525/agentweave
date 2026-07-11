use crate::skill_authoring::build_package_draft;
use crate::skill_management::CreateSkillDraftRequest;
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_state::SkillStateStore;
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStorePaths,
    SkillStoreTestFaults,
};
use crate::storage::Storage;
use tempfile::{TempDir, tempdir};

#[derive(Clone, Copy)]
enum ReplacementTarget {
    RevisionDirectory,
    StagingRoot,
}

struct AuthoringFixture {
    _app: TempDir,
    _cache: TempDir,
    storage: Storage,
    store: SkillRevisionStore,
}

impl AuthoringFixture {
    async fn new(faults: SkillStoreTestFaults) -> Self {
        let app = tempdir().unwrap();
        let cache = tempdir().unwrap();
        let paths = SkillStorePaths::prepare(app.path(), cache.path())
            .await
            .unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let state = SkillStateStore::new(storage.clone());
        let store =
            SkillRevisionStore::with_test_faults(paths, state, SkillStoreLimits::default(), faults);
        Self {
            _app: app,
            _cache: cache,
            storage,
            store,
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revision_replacement_after_snapshot_cannot_bind_a_staging_row() {
    assert_replacement_rejected(
        SkillStoreFaultPoint::StagingAuthorAfterSnapshot,
        ReplacementTarget::RevisionDirectory,
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn staging_root_replacement_after_snapshot_cannot_bind_a_staging_row() {
    assert_replacement_rejected(
        SkillStoreFaultPoint::StagingAuthorAfterSnapshot,
        ReplacementTarget::StagingRoot,
        false,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revision_replacement_after_record_deletes_only_the_inserted_staging_row() {
    assert_replacement_rejected(
        SkillStoreFaultPoint::StagingAuthorAfterRecord,
        ReplacementTarget::RevisionDirectory,
        true,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn staging_root_replacement_after_record_deletes_only_the_inserted_staging_row() {
    assert_replacement_rejected(
        SkillStoreFaultPoint::StagingAuthorAfterRecord,
        ReplacementTarget::StagingRoot,
        true,
    )
    .await;
}

async fn assert_replacement_rejected(
    point: SkillStoreFaultPoint,
    target: ReplacementTarget,
    row_exists_at_gate: bool,
) {
    let revision_id = uuid::Uuid::new_v4().to_string();
    let faults = SkillStoreTestFaults::default();
    faults.set_revision_id_once(&revision_id);
    let gate = faults.gate_once(point);
    let fixture = AuthoringFixture::new(faults).await;
    let request = draft_request();
    let authored = build_package_draft(&request).unwrap();
    let store = fixture.store.clone();
    let package_id = request.package_id.clone();
    let operation = tokio::spawn(async move {
        store
            .create_authored_staging_revision(
                &package_id,
                request.kind,
                authored.files(),
                "owner-1",
            )
            .await
    });
    gate.wait_entered().await;

    assert_eq!(
        revision_count(&fixture.storage, &revision_id).await,
        i64::from(row_exists_at_gate)
    );
    let staging = fixture.store.paths().staging.clone();
    let detached_authored = replace_store_path(&staging, &revision_id, target).await;

    gate.release().await;
    let error = operation.await.unwrap().unwrap_err();
    let message = format!("{error:#}");

    assert!(
        message.contains("identity changed") || message.contains("identity mismatch"),
        "{message}"
    );
    assert_eq!(revision_count(&fixture.storage, &revision_id).await, 0);
    assert_eq!(
        tokio::fs::read(staging.join(&revision_id).join("replacement-marker"))
            .await
            .unwrap(),
        b"replacement"
    );
    if detached_authored.exists() {
        assert!(detached_authored.is_dir());
        assert!(directory_is_empty(&detached_authored).await);
        assert!(
            message.contains("compensation failed"),
            "retained opened tree must be reported for recovery: {message}"
        );
    }
}

async fn replace_store_path(
    staging: &std::path::Path,
    revision_id: &str,
    target: ReplacementTarget,
) -> std::path::PathBuf {
    match target {
        ReplacementTarget::RevisionDirectory => {
            let destination = staging.join(revision_id);
            let detached = staging.join(format!("{revision_id}-detached"));
            tokio::fs::rename(&destination, &detached).await.unwrap();
            tokio::fs::create_dir(&destination).await.unwrap();
            tokio::fs::write(destination.join("replacement-marker"), b"replacement")
                .await
                .unwrap();
            detached
        }
        ReplacementTarget::StagingRoot => {
            let detached_root = staging.with_file_name(format!(
                "{}-detached",
                staging.file_name().unwrap().to_string_lossy()
            ));
            tokio::fs::rename(staging, &detached_root).await.unwrap();
            tokio::fs::create_dir(staging).await.unwrap();
            let replacement = staging.join(revision_id);
            tokio::fs::create_dir(&replacement).await.unwrap();
            tokio::fs::write(replacement.join("replacement-marker"), b"replacement")
                .await
                .unwrap();
            detached_root.join(revision_id)
        }
    }
}

fn draft_request() -> CreateSkillDraftRequest {
    CreateSkillDraftRequest {
        package_id: SkillPackageId::parse("com.example.race").unwrap(),
        display_name: "Race test".into(),
        description: "Authoring identity race test.".into(),
        kind: SkillPackageKind::InstructionOnly,
        required_tools: Vec::new(),
    }
}

async fn revision_count(storage: &Storage, revision_id: &str) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM skill_revisions WHERE revision_id = ?")
        .bind(revision_id)
        .fetch_one(storage.pool())
        .await
        .unwrap()
}

async fn directory_is_empty(path: &std::path::Path) -> bool {
    tokio::fs::read_dir(path)
        .await
        .unwrap()
        .next_entry()
        .await
        .unwrap()
        .is_none()
}
