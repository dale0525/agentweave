use crate::skill_package::SkillPackageId;
use crate::skill_state::{
    NewSkillApproval, NewSkillRevision, SkillApprovalStatus, SkillInstallStatus, SkillLayerRecord,
    SkillRevisionPromotion, SkillRevisionStatus, SkillStateStore,
};
use crate::storage::Storage;
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tempfile::TempDir;
use tokio::sync::{Barrier, mpsc};
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

fn package_id(value: &str) -> SkillPackageId {
    SkillPackageId::parse(value).unwrap()
}

fn revision_input(package_id: SkillPackageId, storage_path: String) -> NewSkillRevision {
    NewSkillRevision {
        package_id,
        version: "1.0.0".into(),
        content_hash: Uuid::new_v4().to_string(),
        storage_path,
        descriptor_json: json!({"schemaVersion": 1}),
        validation_json: json!({"ok": true}),
        created_by: "owner-1".into(),
    }
}

fn approval_input(package_id: SkillPackageId) -> NewSkillApproval {
    NewSkillApproval {
        package_id,
        revision_id: "rev-1".into(),
        operation: "activate".into(),
        requested_by: "owner-1".into(),
        permission_diff: json!([]),
    }
}

fn file_database() -> (TempDir, String) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("skill-state.db");
    let url = format!("sqlite://{}?mode=rwc", path.display());
    (directory, url)
}

async fn await_operation_entries(receiver: &mut mpsc::UnboundedReceiver<()>) {
    for _ in 0..2 {
        timeout(Duration::from_secs(1), receiver.recv())
            .await
            .expect("competitor did not reach the operation entry")
            .expect("operation entry channel closed");
    }
}

async fn await_task<T>(task: tokio::task::JoinHandle<T>) -> T {
    timeout(Duration::from_secs(2), task)
        .await
        .expect("competitor did not finish")
        .expect("competitor task panicked")
}

#[tokio::test]
async fn authoritative_revision_id_drives_staging_promotion_and_activation() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let package_id = package_id("com.example.calendar");
    let revision_id = SkillStateStore::allocate_revision_id();
    let staging_path = format!("staging/{revision_id}");
    let revision = state
        .create_staging_revision_record(
            &revision_id,
            revision_input(package_id.clone(), staging_path.clone()),
        )
        .await
        .unwrap();

    assert_eq!(revision.revision_id, revision_id);
    assert_eq!(revision.storage_path, staging_path);
    assert_eq!(revision.status, SkillRevisionStatus::Staging);
    assert!(
        state
            .activate_revision(
                &package_id,
                &revision_id,
                SkillLayerRecord::Managed,
                "owner-1",
            )
            .await
            .is_err()
    );

    let managed_path = format!("managed/{}/revisions/{revision_id}", package_id.as_str());
    let promoted = state
        .promote_revision_record(&revision_id, &managed_path)
        .await
        .unwrap();
    assert_eq!(promoted.status, SkillRevisionStatus::Managed);
    assert_eq!(promoted.storage_path, managed_path);
    state
        .record_revision_activation(
            &package_id,
            &revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();

    let installation = state.get_installation(&package_id).await.unwrap().unwrap();
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(revision_id.as_str())
    );
    let direct = state
        .create_revision(revision_input(
            package_id,
            "managed/com.example.calendar/revisions/direct".into(),
        ))
        .await
        .unwrap();
    assert_eq!(direct.status, SkillRevisionStatus::Managed);
}

#[tokio::test]
async fn promotion_atomically_refreshes_final_revision_metadata() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let package_id = package_id("com.example.calendar");
    let revision_id = SkillStateStore::allocate_revision_id();
    state
        .create_staging_revision_record(
            &revision_id,
            revision_input(package_id.clone(), format!("staging/{revision_id}")),
        )
        .await
        .unwrap();
    let descriptor = json!({
        "schemaVersion": 1,
        "id": package_id.as_str(),
        "version": "2.0.0"
    });
    let validation = json!({"status": "valid", "checkedAtPromotion": true});
    let managed_path = format!("managed/{}/revisions/{revision_id}", package_id.as_str());

    let promoted = state
        .promote_revision_record_with_metadata(
            &revision_id,
            SkillRevisionPromotion {
                version: "2.0.0".into(),
                content_hash: "final-content-hash".into(),
                storage_path: managed_path.clone(),
                descriptor_json: descriptor.clone(),
                validation_json: validation.clone(),
            },
        )
        .await
        .unwrap();

    assert_eq!(promoted.version, "2.0.0");
    assert_eq!(promoted.content_hash, "final-content-hash");
    assert_eq!(promoted.storage_path, managed_path);
    assert_eq!(promoted.descriptor_json, descriptor);
    assert_eq!(promoted.validation_json, validation);
    assert_eq!(promoted.status, SkillRevisionStatus::Managed);
    assert_eq!(
        state.get_revision(&revision_id).await.unwrap(),
        Some(promoted)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn compatibility_promotion_does_not_overwrite_metadata_updated_before_transaction() {
    let (_directory, url) = file_database();
    let storage = Storage::connect(&url).await.unwrap();
    let lock_storage = Storage::connect(&url).await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let revision_id = SkillStateStore::allocate_revision_id();
    state
        .create_staging_revision_record(
            &revision_id,
            revision_input(
                package_id("com.example.compatibility"),
                format!("staging/{revision_id}"),
            ),
        )
        .await
        .unwrap();
    let mut lock = lock_storage.pool().acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let promotion_state = state.clone();
    let promotion_revision = revision_id.clone();
    let promotion = tokio::spawn(async move {
        promotion_state
            .promote_revision_record(&promotion_revision, "managed/compatibility")
            .await
    });
    sleep(Duration::from_millis(50)).await;
    assert!(!promotion.is_finished());
    sqlx::query(
        r#"UPDATE skill_revisions
           SET version = '2.0.0', content_hash = 'new-hash',
               descriptor_json = '{"version":"2.0.0"}',
               validation_json = '{"status":"new"}'
           WHERE revision_id = ?"#,
    )
    .bind(&revision_id)
    .execute(&mut *lock)
    .await
    .unwrap();
    sqlx::query("COMMIT").execute(&mut *lock).await.unwrap();
    drop(lock);

    promotion.await.unwrap().unwrap();
    let record = SkillStateStore::new(storage)
        .get_revision(&revision_id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(record.version, "2.0.0");
    assert_eq!(record.content_hash, "new-hash");
    assert_eq!(record.descriptor_json, json!({"version": "2.0.0"}));
    assert_eq!(record.validation_json, json!({"status": "new"}));
    assert_eq!(record.storage_path, "managed/compatibility");
    assert_eq!(record.status, SkillRevisionStatus::Managed);
}

#[tokio::test]
async fn create_revision_is_a_trusted_managed_import_compatibility_contract() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let package_id = package_id("com.example.trusted");

    let revision = state
        .create_revision(revision_input(
            package_id.clone(),
            "managed/com.example.trusted/revisions/imported".into(),
        ))
        .await
        .unwrap();

    assert_eq!(revision.status, SkillRevisionStatus::Managed);
    state
        .activate_revision(
            &package_id,
            &revision.revision_id,
            SkillLayerRecord::Managed,
            "trusted-importer",
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn staging_revision_rejects_non_v4_authoritative_id_without_writing() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());

    let error = state
        .create_staging_revision_record(
            "rev-1",
            revision_input(package_id("com.example.calendar"), "staging/rev-1".into()),
        )
        .await
        .unwrap_err();

    assert!(error.to_string().contains("UUID v4"));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_revisions")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn public_uuid_key_operations_reject_invalid_absent_ids() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);

    assert!(state.get_revision("rev-1").await.is_err());
    assert!(state.get_approval("approval-1").await.is_err());
    assert!(state.approve("approval-1", "owner-2").await.is_err());
    assert!(state.reject("approval-1", "owner-2").await.is_err());
    assert!(state.get_circuit_state("rev-1").await.is_err());
}

#[tokio::test]
async fn promotion_explicitly_rejects_unknown_lifecycle_value() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let revision_id = Uuid::new_v4().to_string();
    let mut connection = storage.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO skill_revisions
           (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
            validation_json, created_by, created_at, lifecycle_status)
           VALUES (?, 'com.example.calendar', '1.0.0', 'hash', 'path', '{}', '{}',
                   'owner-1', '2026-01-01T00:00:00Z', 'future')"#,
    )
    .bind(&revision_id)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);

    let error = state
        .promote_revision_record(&revision_id, "managed/revision")
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unknown skill revision status"));
}

#[tokio::test]
async fn lifecycle_transitions_reject_invalid_sources_and_quarantine_new_path() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let package_id = package_id("com.example.calendar");
    let revision = state
        .create_revision(revision_input(
            package_id.clone(),
            "managed/com.example.calendar/revisions/direct".into(),
        ))
        .await
        .unwrap();

    assert!(
        state
            .promote_revision_record(&revision.revision_id, "managed/other")
            .await
            .is_err()
    );
    let quarantined_path = format!("quarantine/{}", revision.revision_id);
    let quarantined = state
        .quarantine_revision_record(
            &revision.revision_id,
            &quarantined_path,
            "signature revoked",
        )
        .await
        .unwrap();
    assert_eq!(quarantined.status, SkillRevisionStatus::Quarantined);
    assert_eq!(quarantined.storage_path, quarantined_path);
    assert_eq!(
        quarantined.validation_json["quarantineReason"],
        "signature revoked"
    );
    assert!(
        state
            .activate_revision(
                &package_id,
                &revision.revision_id,
                SkillLayerRecord::Managed,
                "owner-1",
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn revision_activation_alias_and_existing_entry_share_contract() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let package_id = package_id("com.example.calendar");
    let first = state
        .create_revision(revision_input(
            package_id.clone(),
            "managed/com.example.calendar/revisions/first".into(),
        ))
        .await
        .unwrap();
    let second = state
        .create_revision(revision_input(
            package_id.clone(),
            "managed/com.example.calendar/revisions/second".into(),
        ))
        .await
        .unwrap();

    state
        .record_revision_activation(
            &package_id,
            &first.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();
    state
        .activate_revision(
            &package_id,
            &second.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();

    let installation = state.get_installation(&package_id).await.unwrap().unwrap();
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(second.revision_id.as_str())
    );
    let audit = state.list_audit(&package_id).await.unwrap();
    assert_eq!(
        audit
            .iter()
            .filter(|entry| entry.operation == "activate_revision")
            .count(),
        2
    );
}

#[tokio::test]
async fn existing_installation_upsert_rolls_back_when_audit_fails() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let package_id = package_id("com.example.calendar");
    let first = state
        .create_revision(revision_input(
            package_id.clone(),
            "managed/com.example.calendar/revisions/first".into(),
        ))
        .await
        .unwrap();
    let second = state
        .create_revision(revision_input(
            package_id.clone(),
            "managed/com.example.calendar/revisions/second".into(),
        ))
        .await
        .unwrap();
    state
        .activate_revision(
            &package_id,
            &first.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();
    let trigger = format!(
        r#"CREATE TRIGGER fail_second_revision_audit
           BEFORE INSERT ON skill_audit_log
           WHEN NEW.operation = 'activate_revision' AND NEW.revision_id = '{}'
           BEGIN
             SELECT RAISE(ABORT, 'audit failed');
           END"#,
        second.revision_id
    );
    sqlx::query(&trigger).execute(storage.pool()).await.unwrap();

    assert!(
        state
            .activate_revision(
                &package_id,
                &second.revision_id,
                SkillLayerRecord::Managed,
                "owner-1",
            )
            .await
            .is_err()
    );

    let installation = state.get_installation(&package_id).await.unwrap().unwrap();
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(first.revision_id.as_str())
    );
    assert_eq!(state.list_audit(&package_id).await.unwrap().len(), 1);
}

#[tokio::test]
async fn quarantine_failure_rolls_back_revision_path_status_installation_and_json() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let package_id = package_id("com.example.calendar");
    let original_path = "managed/com.example.calendar/revisions/first";
    let revision = state
        .create_revision(revision_input(package_id.clone(), original_path.into()))
        .await
        .unwrap();
    state
        .activate_revision(
            &package_id,
            &revision.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();
    sqlx::query(
        r#"CREATE TRIGGER fail_quarantine_audit
           BEFORE INSERT ON skill_audit_log
           WHEN NEW.operation = 'mark_revision_quarantined'
           BEGIN
             SELECT RAISE(ABORT, 'audit failed');
           END"#,
    )
    .execute(storage.pool())
    .await
    .unwrap();

    assert!(
        state
            .quarantine_revision_record(
                &revision.revision_id,
                "quarantine/revision",
                "signature revoked",
            )
            .await
            .is_err()
    );

    let loaded = state
        .get_revision(&revision.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.status, SkillRevisionStatus::Managed);
    assert_eq!(loaded.storage_path, original_path);
    assert_eq!(loaded.validation_json, json!({"ok": true}));
    let installation = state.get_installation(&package_id).await.unwrap().unwrap();
    assert_eq!(installation.status, SkillInstallStatus::Active);
    assert!(installation.enabled);
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(revision.revision_id.as_str())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_approval_resolution_has_one_winner_and_business_loser() {
    let (_directory, url) = file_database();
    let first_storage = Storage::connect(&url).await.unwrap();
    let second_storage = Storage::connect(&url).await.unwrap();
    let lock_storage = Storage::connect(&url).await.unwrap();
    let first = SkillStateStore::new(first_storage.clone());
    let second = SkillStateStore::new(second_storage);
    let approval = first
        .create_approval(approval_input(package_id("com.example.calendar")))
        .await
        .unwrap();
    let approval_id = approval.approval_id.clone();
    let second_id = approval.approval_id.clone();
    let mut lock = lock_storage.pool().acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let (entered, mut entries) = mpsc::unbounded_channel();
    let approve_barrier = barrier.clone();
    let approve_entered = entered.clone();
    let approve = tokio::spawn(async move {
        approve_barrier.wait().await;
        approve_entered.send(()).unwrap();
        first.approve(&approval_id, "owner-2").await
    });
    let reject_barrier = barrier.clone();
    let reject_entered = entered.clone();
    let reject = tokio::spawn(async move {
        reject_barrier.wait().await;
        reject_entered.send(()).unwrap();
        second.reject(&second_id, "owner-3").await
    });
    drop(entered);
    barrier.wait().await;
    await_operation_entries(&mut entries).await;
    assert!(!approve.is_finished());
    assert!(!reject.is_finished());
    sqlx::query("COMMIT").execute(&mut *lock).await.unwrap();
    drop(lock);

    let approve = await_task(approve).await;
    let reject = await_task(reject).await;

    assert_eq!(approve.is_ok() as u8 + reject.is_ok() as u8, 1);
    let loser = approve.err().or_else(|| reject.err()).unwrap().to_string();
    assert!(
        loser.contains("already resolved"),
        "unexpected loser error: {loser}"
    );
    assert!(!loser.contains("database is locked"));
    let resolved = SkillStateStore::new(first_storage)
        .get_approval(&approval.approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(resolved.status, SkillApprovalStatus::Pending);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_quarantine_has_one_business_winner_without_sqlite_busy() {
    let (_directory, url) = file_database();
    let first_storage = Storage::connect(&url).await.unwrap();
    let second_storage = Storage::connect(&url).await.unwrap();
    let lock_storage = Storage::connect(&url).await.unwrap();
    let package_id = package_id("com.example.calendar");
    let first = SkillStateStore::new(first_storage.clone());
    let second = SkillStateStore::new(second_storage);
    let revision = first
        .create_revision(revision_input(
            package_id.clone(),
            "managed/com.example.calendar/revisions/revision".into(),
        ))
        .await
        .unwrap();
    first
        .activate_revision(
            &package_id,
            &revision.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();

    let mut lock = lock_storage.pool().acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let (entered, mut entries) = mpsc::unbounded_channel();
    let first_barrier = barrier.clone();
    let first_entered = entered.clone();
    let first_revision = revision.revision_id.clone();
    let first_task = tokio::spawn(async move {
        first_barrier.wait().await;
        first_entered.send(()).unwrap();
        first
            .quarantine_revision_record(&first_revision, "quarantine/first", "first")
            .await
    });
    let second_barrier = barrier.clone();
    let second_entered = entered.clone();
    let second_revision = revision.revision_id.clone();
    let second_task = tokio::spawn(async move {
        second_barrier.wait().await;
        second_entered.send(()).unwrap();
        second
            .quarantine_revision_record(&second_revision, "quarantine/second", "second")
            .await
    });
    drop(entered);
    barrier.wait().await;
    await_operation_entries(&mut entries).await;
    assert!(!first_task.is_finished());
    assert!(!second_task.is_finished());
    sqlx::query("COMMIT").execute(&mut *lock).await.unwrap();
    drop(lock);

    let first_result = await_task(first_task).await;
    let second_result = await_task(second_task).await;
    assert_eq!(first_result.is_ok() as u8 + second_result.is_ok() as u8, 1);
    let winner_path = first_result
        .as_ref()
        .ok()
        .or_else(|| second_result.as_ref().ok())
        .unwrap()
        .storage_path
        .clone();
    let loser = first_result
        .err()
        .or_else(|| second_result.err())
        .unwrap()
        .to_string();
    assert!(loser.contains("quarantined"), "unexpected loser: {loser}");
    assert!(!loser.contains("database is locked"));
    assert!(!loser.contains("SQLITE_BUSY"));

    let state = SkillStateStore::new(first_storage);
    let stored = state
        .get_revision(&revision.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, SkillRevisionStatus::Quarantined);
    assert_eq!(stored.storage_path, winner_path);
    let installation = state.get_installation(&package_id).await.unwrap().unwrap();
    assert_eq!(installation.status, SkillInstallStatus::Quarantined);
    assert!(!installation.enabled);
    assert!(installation.active_revision_id.is_none());
    let quarantine_audits = state
        .list_audit(&package_id)
        .await
        .unwrap()
        .into_iter()
        .filter(|entry| entry.operation == "mark_revision_quarantined")
        .count();
    assert_eq!(quarantine_audits, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_promotion_has_one_business_winner_without_sqlite_busy() {
    let (_directory, url) = file_database();
    let first_storage = Storage::connect(&url).await.unwrap();
    let second_storage = Storage::connect(&url).await.unwrap();
    let lock_storage = Storage::connect(&url).await.unwrap();
    let first = SkillStateStore::new(first_storage.clone());
    let second = SkillStateStore::new(second_storage);
    let revision_id = SkillStateStore::allocate_revision_id();
    first
        .create_staging_revision_record(
            &revision_id,
            revision_input(
                package_id("com.example.calendar"),
                format!("staging/{revision_id}"),
            ),
        )
        .await
        .unwrap();

    let mut lock = lock_storage.pool().acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let (entered, mut entries) = mpsc::unbounded_channel();
    let first_barrier = barrier.clone();
    let first_entered = entered.clone();
    let first_revision = revision_id.clone();
    let first_task = tokio::spawn(async move {
        first_barrier.wait().await;
        first_entered.send(()).unwrap();
        first
            .promote_revision_record(&first_revision, "managed/first")
            .await
    });
    let second_barrier = barrier.clone();
    let second_entered = entered.clone();
    let second_revision = revision_id.clone();
    let second_task = tokio::spawn(async move {
        second_barrier.wait().await;
        second_entered.send(()).unwrap();
        second
            .promote_revision_record(&second_revision, "managed/second")
            .await
    });
    drop(entered);
    barrier.wait().await;
    await_operation_entries(&mut entries).await;
    assert!(!first_task.is_finished());
    assert!(!second_task.is_finished());
    sqlx::query("COMMIT").execute(&mut *lock).await.unwrap();
    drop(lock);

    let first_result = await_task(first_task).await;
    let second_result = await_task(second_task).await;
    assert_eq!(first_result.is_ok() as u8 + second_result.is_ok() as u8, 1);
    let winner_path = first_result
        .as_ref()
        .ok()
        .or_else(|| second_result.as_ref().ok())
        .unwrap()
        .storage_path
        .clone();
    let loser = first_result
        .err()
        .or_else(|| second_result.err())
        .unwrap()
        .to_string();
    assert!(loser.contains("cannot be promoted from managed"), "{loser}");
    assert!(!loser.contains("database is locked"));
    assert!(!loser.contains("SQLITE_BUSY"));
    let stored = SkillStateStore::new(first_storage)
        .get_revision(&revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, SkillRevisionStatus::Managed);
    assert_eq!(stored.storage_path, winner_path);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_activation_has_business_winner_and_wrong_package_loser_without_sqlite_busy() {
    let (_directory, url) = file_database();
    let first_storage = Storage::connect(&url).await.unwrap();
    let second_storage = Storage::connect(&url).await.unwrap();
    let lock_storage = Storage::connect(&url).await.unwrap();
    let correct_package = package_id("com.example.calendar");
    let wrong_package = package_id("com.example.mail");
    let first = SkillStateStore::new(first_storage.clone());
    let second = SkillStateStore::new(second_storage);
    let revision = first
        .create_revision(revision_input(
            correct_package.clone(),
            "managed/com.example.calendar/revisions/race".into(),
        ))
        .await
        .unwrap();

    let mut lock = lock_storage.pool().acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let (entered, mut entries) = mpsc::unbounded_channel();
    let winner_barrier = barrier.clone();
    let winner_entered = entered.clone();
    let winner_package = correct_package.clone();
    let winner_revision = revision.revision_id.clone();
    let winner = tokio::spawn(async move {
        winner_barrier.wait().await;
        winner_entered.send(()).unwrap();
        first
            .activate_revision(
                &winner_package,
                &winner_revision,
                SkillLayerRecord::Managed,
                "owner-1",
            )
            .await
    });
    let loser_barrier = barrier.clone();
    let loser_entered = entered.clone();
    let loser_package = wrong_package.clone();
    let loser_revision = revision.revision_id.clone();
    let loser = tokio::spawn(async move {
        loser_barrier.wait().await;
        loser_entered.send(()).unwrap();
        second
            .activate_revision(
                &loser_package,
                &loser_revision,
                SkillLayerRecord::Managed,
                "owner-2",
            )
            .await
    });
    drop(entered);
    barrier.wait().await;
    await_operation_entries(&mut entries).await;
    assert!(!winner.is_finished());
    assert!(!loser.is_finished());
    sqlx::query("COMMIT").execute(&mut *lock).await.unwrap();
    drop(lock);

    let winner = await_task(winner).await;
    let loser = await_task(loser).await;
    assert!(winner.is_ok(), "correct-package activation failed");
    let loser = loser.unwrap_err();
    let message = format!("{loser:#}");
    assert!(message.contains("belongs to com.example.calendar, not com.example.mail"));
    assert!(!message.contains("database is locked"));
    assert!(!message.contains("SQLITE_BUSY"));

    let state = SkillStateStore::new(first_storage);
    let installation = state
        .get_installation(&correct_package)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(revision.revision_id.as_str())
    );
    assert!(
        state
            .get_installation(&wrong_package)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(state.list_audit(&correct_package).await.unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_busy_timeout_is_configured_and_waits_for_a_held_write_lock() {
    let (_directory, url) = file_database();
    let first = Storage::connect(&url).await.unwrap();
    let second = Storage::connect(&url).await.unwrap();
    let busy_timeout: i64 = sqlx::query_scalar("PRAGMA busy_timeout")
        .fetch_one(second.pool())
        .await
        .unwrap();
    assert_eq!(busy_timeout, 5_000);

    let mut lock = first.pool().acquire().await.unwrap();
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *lock)
        .await
        .unwrap();
    let started = Instant::now();
    let write = tokio::spawn(async move {
        sqlx::query("INSERT INTO runtime_settings (key, value) VALUES ('busy-timeout-test', 'ok')")
            .execute(second.pool())
            .await
    });
    sleep(Duration::from_millis(150)).await;
    assert!(!write.is_finished());
    sqlx::query("COMMIT").execute(&mut *lock).await.unwrap();
    drop(lock);

    write.await.unwrap().unwrap();
    assert!(started.elapsed() >= Duration::from_millis(100));
}
