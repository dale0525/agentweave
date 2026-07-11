use crate::skill_source::hash_package_tree;
use crate::skill_state::{SkillRevisionMetadata, SkillRevisionStatus, SkillStateStore};
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
    let attempt = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::RevisionLockAttempt);
    let promoter = fixture.store.clone();
    let promote_revision = staged.revision_id.clone();
    let promote = tokio::spawn(async move { promoter.promote_revision(&promote_revision).await });
    attempt.wait_entered().await;
    attempt.release().await;
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
    let second_attempt = second_faults.gate_once(SkillStoreFaultPoint::RevisionLockAttempt);
    let first_revision = staged.revision_id.clone();
    let first = tokio::spawn(async move { first_store.promote_revision(&first_revision).await });
    first_gate.wait_entered().await;
    let second_revision = staged.revision_id.clone();
    let second = tokio::spawn(async move { second_store.promote_revision(&second_revision).await });
    second_attempt.wait_entered().await;
    second_attempt.release().await;
    assert!(!second_gate.has_entered());

    first_gate.release().await;
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

#[tokio::test]
async fn independent_store_write_blocks_promotion_and_acknowledged_edit_is_promoted() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let writer_faults = SkillStoreTestFaults::default();
    let promoter_faults = SkillStoreTestFaults::default();
    let writer = independent_store(&fixture, writer_faults.clone());
    let promoter = independent_store(&fixture, promoter_faults.clone());
    let writer_gate = writer_faults.gate_once(SkillStoreFaultPoint::WriteAfterLock);
    let promoter_gate = promoter_faults.gate_once(SkillStoreFaultPoint::PromoteAfterLock);
    let promoter_attempt = promoter_faults.gate_once(SkillStoreFaultPoint::RevisionLockAttempt);
    let write_revision = staged.revision_id.clone();
    let write = tokio::spawn(async move {
        writer
            .write_staging_file(
                &write_revision,
                Path::new("SKILL.md"),
                b"---\nname: lock\ndescription: lock\n---\nindependent edit\n",
            )
            .await
    });
    writer_gate.wait_entered().await;
    let promote_revision = staged.revision_id.clone();
    let promote = tokio::spawn(async move { promoter.promote_revision(&promote_revision).await });
    promoter_attempt.wait_entered().await;
    promoter_attempt.release().await;
    assert!(!promoter_gate.has_entered());
    writer_gate.release().await;
    let write_result = write.await.unwrap();
    promoter_gate.wait_entered().await;
    promoter_gate.release().await;
    let managed = promote.await.unwrap();

    write_result.unwrap();
    let managed = managed.unwrap();
    let content = tokio::fs::read_to_string(managed.path.join("SKILL.md"))
        .await
        .unwrap();
    assert!(content.contains("independent edit"));
}

#[tokio::test]
async fn independent_store_write_blocks_quarantine_and_acknowledged_edit_is_quarantined() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let writer_faults = SkillStoreTestFaults::default();
    let quarantine_faults = SkillStoreTestFaults::default();
    let writer = independent_store(&fixture, writer_faults.clone());
    let quarantiner = independent_store(&fixture, quarantine_faults.clone());
    let writer_gate = writer_faults.gate_once(SkillStoreFaultPoint::WriteAfterLock);
    let quarantine_gate = quarantine_faults.gate_once(SkillStoreFaultPoint::QuarantineAfterLock);
    let quarantine_attempt = quarantine_faults.gate_once(SkillStoreFaultPoint::RevisionLockAttempt);
    let write_revision = staged.revision_id.clone();
    let write = tokio::spawn(async move {
        writer
            .write_staging_file(&write_revision, Path::new("ack.txt"), b"acknowledged")
            .await
    });
    writer_gate.wait_entered().await;
    let quarantine_revision = staged.revision_id.clone();
    let quarantine = tokio::spawn(async move {
        quarantiner
            .quarantine_revision(&quarantine_revision, "concurrent")
            .await
    });
    quarantine_attempt.wait_entered().await;
    quarantine_attempt.release().await;
    assert!(!quarantine_gate.has_entered());
    writer_gate.release().await;
    let write_result = write.await.unwrap();
    quarantine_gate.wait_entered().await;
    quarantine_gate.release().await;
    let quarantined = quarantine.await.unwrap();

    write_result.unwrap();
    let quarantined = quarantined.unwrap();
    assert_eq!(
        tokio::fs::read(quarantined.path.join("ack.txt"))
            .await
            .unwrap(),
        b"acknowledged"
    );
}

#[tokio::test]
async fn independent_store_double_write_is_serialized_and_final_hash_matches_tree() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let first_faults = SkillStoreTestFaults::default();
    let second_faults = SkillStoreTestFaults::default();
    let first_store = independent_store(&fixture, first_faults.clone());
    let second_store = independent_store(&fixture, second_faults.clone());
    let first_gate = first_faults.gate_once(SkillStoreFaultPoint::WriteAfterLock);
    let second_gate = second_faults.gate_once(SkillStoreFaultPoint::WriteAfterLock);
    let second_attempt = second_faults.gate_once(SkillStoreFaultPoint::RevisionLockAttempt);
    let first_revision = staged.revision_id.clone();
    let first = tokio::spawn(async move {
        first_store
            .write_staging_file(&first_revision, Path::new("first.txt"), b"first")
            .await
    });
    first_gate.wait_entered().await;
    let second_revision = staged.revision_id.clone();
    let second = tokio::spawn(async move {
        second_store
            .write_staging_file(&second_revision, Path::new("second.txt"), b"second")
            .await
    });
    second_attempt.wait_entered().await;
    second_attempt.release().await;
    assert!(!second_gate.has_entered());
    first_gate.release().await;
    let first_result = first.await.unwrap();
    second_gate.wait_entered().await;
    second_gate.release().await;
    let second_result = second.await.unwrap();

    first_result.unwrap();
    second_result.unwrap();
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert!(staged.path.join("first.txt").is_file());
    assert!(staged.path.join("second.txt").is_file());
    assert_eq!(
        record.content_hash,
        hash_package_tree(&staged.path).await.unwrap()
    );
}

#[tokio::test]
async fn promotion_rejects_db_metadata_changed_after_locked_observation() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let faults = SkillStoreTestFaults::default();
    let store = independent_store(&fixture, faults.clone());
    let gate = faults.gate_once(SkillStoreFaultPoint::PromoteBeforeDestinationCommit);
    let revision_id = staged.revision_id.clone();
    let promotion = tokio::spawn(async move { store.promote_revision(&revision_id).await });
    gate.wait_entered().await;
    fixture
        .state
        .refresh_staging_revision_metadata(
            &staged.revision_id,
            SkillRevisionMetadata {
                version: "9.0.0".into(),
                content_hash: "external-hash".into(),
                descriptor_json: json!({"external": true}),
                validation_json: json!({"status": "external"}),
            },
        )
        .await
        .unwrap();
    gate.release().await;

    let error = promotion.await.unwrap().unwrap_err();

    assert!(format!("{error:#}").contains("changed since operation observation"));
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Staging);
    assert_eq!(record.content_hash, "external-hash");
    assert!(staged.path.is_dir());
}

#[tokio::test]
async fn quarantine_rejects_db_metadata_changed_after_locked_observation() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let faults = SkillStoreTestFaults::default();
    let store = independent_store(&fixture, faults.clone());
    let gate = faults.gate_once(SkillStoreFaultPoint::QuarantineAfterLock);
    let revision_id = staged.revision_id.clone();
    let quarantine = tokio::spawn(async move {
        store
            .quarantine_revision(&revision_id, "stale observation")
            .await
    });
    gate.wait_entered().await;
    fixture
        .state
        .refresh_staging_revision_metadata(
            &staged.revision_id,
            SkillRevisionMetadata {
                version: "9.0.0".into(),
                content_hash: "external-hash".into(),
                descriptor_json: json!({"external": true}),
                validation_json: json!({"status": "external"}),
            },
        )
        .await
        .unwrap();
    gate.release().await;

    let error = quarantine.await.unwrap().unwrap_err();

    assert!(format!("{error:#}").contains("changed since operation observation"));
    let record = fixture
        .state
        .get_revision(&staged.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.status, SkillRevisionStatus::Staging);
    assert_eq!(record.content_hash, "external-hash");
    assert!(staged.path.is_dir());
}

#[tokio::test]
async fn staging_write_cas_rejects_metadata_changed_after_file_commit() {
    let fixture = ConcurrentFixture::new().await;
    let staged = fixture.stage().await;
    let original = tokio::fs::read(staged.path.join("SKILL.md")).await.unwrap();
    let faults = SkillStoreTestFaults::default();
    let store = independent_store(&fixture, faults.clone());
    let gate = faults.gate_once(SkillStoreFaultPoint::WriteBeforeMetadataCommit);
    let revision_id = staged.revision_id.clone();
    let write = tokio::spawn(async move {
        store
            .write_staging_file(
                &revision_id,
                Path::new("SKILL.md"),
                b"---\nname: lock\ndescription: changed\n---\nchanged\n",
            )
            .await
    });
    gate.wait_entered().await;
    fixture
        .state
        .refresh_staging_revision_metadata(
            &staged.revision_id,
            SkillRevisionMetadata {
                version: "9.0.0".into(),
                content_hash: "external-hash".into(),
                descriptor_json: json!({"external": true}),
                validation_json: json!({"status": "external"}),
            },
        )
        .await
        .unwrap();
    gate.release().await;

    let error = write.await.unwrap().unwrap_err();

    assert!(format!("{error:#}").contains("changed since operation observation"));
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
    assert_eq!(record.content_hash, "external-hash");
}

fn independent_store(
    fixture: &ConcurrentFixture,
    faults: SkillStoreTestFaults,
) -> SkillRevisionStore {
    SkillRevisionStore::with_test_faults(
        fixture.paths.clone(),
        fixture.state.clone(),
        SkillStoreLimits {
            max_file_bytes: 1024,
            max_package_bytes: 4096,
            ..SkillStoreLimits::default()
        },
        faults,
    )
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
