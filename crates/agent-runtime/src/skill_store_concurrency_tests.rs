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

struct ConcurrentFixture {
    _app: TempDir,
    _cache: TempDir,
    state: SkillStateStore,
    paths: SkillStorePaths,
    store: SkillRevisionStore,
    faults: SkillStoreTestFaults,
}

impl ConcurrentFixture {
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
                max_file_bytes: 1024,
                max_package_bytes: 4096,
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

    async fn stage(&self) -> crate::skill_store::StoredSkillRevision {
        let source = write_package().await;
        self.store
            .create_staging_revision(source.path(), "owner-1")
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn acknowledged_write_completes_before_waiting_promotion_on_store_clone() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::WriteAfterLock);
    let writer = fixture.store.clone();
    let write_revision = staged.revision_id.clone();
    let write = tokio::spawn(async move {
        writer
            .write_staging_file(
                &write_revision,
                Path::new("SKILL.md"),
                b"---\nname: lock\ndescription: lock\n---\nacknowledged edit\n",
            )
            .await
    });
    gate.wait_entered().await;
    let promoter = fixture.store.clone();
    let promote_revision = staged.revision_id.clone();
    let promote = tokio::spawn(async move { promoter.promote_revision(&promote_revision).await });
    tokio::task::yield_now().await;
    assert!(!promote.is_finished());

    gate.release().await;
    write.await.unwrap().unwrap();
    let managed = promote.await.unwrap().unwrap();

    let content = tokio::fs::read_to_string(managed.path.join("SKILL.md"))
        .await
        .unwrap();
    assert!(content.contains("acknowledged edit"));
    assert_eq!(
        managed.content_hash,
        hash_package_tree(&managed.path).await.unwrap()
    );
}

#[tokio::test]
async fn concurrent_promote_and_quarantine_have_one_business_winner() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::PromoteAfterLock);
    let promoter = fixture.store.clone();
    let promote_revision = staged.revision_id.clone();
    let promote = tokio::spawn(async move { promoter.promote_revision(&promote_revision).await });
    gate.wait_entered().await;
    let quarantiner = fixture.store.clone();
    let quarantine_revision = staged.revision_id.clone();
    let quarantine = tokio::spawn(async move {
        quarantiner
            .quarantine_revision(&quarantine_revision, "concurrent")
            .await
    });

    gate.release().await;
    let promote_result = promote.await.unwrap();
    let quarantine_result = quarantine.await.unwrap();

    assert!(promote_result.is_ok());
    let loser = quarantine_result.unwrap_err().to_string();
    assert!(
        loser.contains("changed while waiting for revision lock"),
        "{loser}"
    );
    assert_eq!(
        fixture
            .state
            .get_revision(&staged.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Managed
    );
}

#[tokio::test]
async fn concurrent_double_promotion_has_one_business_winner() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let gate = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::PromoteAfterLock);
    let first_store = fixture.store.clone();
    let first_revision = staged.revision_id.clone();
    let first = tokio::spawn(async move { first_store.promote_revision(&first_revision).await });
    gate.wait_entered().await;
    let second_store = fixture.store.clone();
    let second_revision = staged.revision_id.clone();
    let second = tokio::spawn(async move { second_store.promote_revision(&second_revision).await });

    gate.release().await;
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_eq!(first.is_ok() as u8 + second.is_ok() as u8, 1);
    let loser = first.err().or_else(|| second.err()).unwrap().to_string();
    assert!(
        loser.contains("changed while waiting for revision lock"),
        "{loser}"
    );
}

#[tokio::test]
async fn independent_store_instances_use_destination_reservation_and_db_cas() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let first_faults = SkillStoreTestFaults::default();
    let second_faults = SkillStoreTestFaults::default();
    let limits = SkillStoreLimits {
        max_file_bytes: 1024,
        max_package_bytes: 4096,
        ..SkillStoreLimits::default()
    };
    let first_store = SkillRevisionStore::with_test_faults(
        fixture.paths.clone(),
        fixture.state.clone(),
        limits,
        first_faults.clone(),
    );
    let second_store = SkillRevisionStore::with_test_faults(
        fixture.paths.clone(),
        fixture.state.clone(),
        limits,
        second_faults.clone(),
    );
    let first_gate = first_faults.gate_once(SkillStoreFaultPoint::PromoteBeforeDestinationCommit);
    let second_gate = second_faults.gate_once(SkillStoreFaultPoint::PromoteBeforeDestinationCommit);
    let first_revision = staged.revision_id.clone();
    let second_revision = staged.revision_id.clone();
    let first = tokio::spawn(async move { first_store.promote_revision(&first_revision).await });
    let second = tokio::spawn(async move { second_store.promote_revision(&second_revision).await });
    first_gate.wait_entered().await;
    second_gate.wait_entered().await;

    first_gate.release().await;
    second_gate.release().await;
    let first = first.await.unwrap();
    let second = second.await.unwrap();

    assert_eq!(first.is_ok() as u8 + second.is_ok() as u8, 1);
    let winner = first.ok().or_else(|| second.ok()).unwrap();
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Managed);
    assert_eq!(record.storage_path, winner.path.to_string_lossy());
    assert_eq!(
        record.content_hash,
        hash_package_tree(&winner.path).await.unwrap()
    );
}

async fn write_package() -> TempDir {
    let root = tempdir().unwrap();
    tokio::fs::write(
        root.path().join("general-agent.json"),
        json!({
            "schemaVersion": 1,
            "id": "com.example.lock",
            "version": "1.0.0",
            "displayName": "lock",
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
        "---\nname: lock\ndescription: lock\n---\noriginal\n",
    )
    .await
    .unwrap();
    root
}
