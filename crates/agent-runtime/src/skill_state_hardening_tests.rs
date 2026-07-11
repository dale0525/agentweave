use crate::skill_package::SkillPackageId;
use crate::skill_state::{
    NewSkillApproval, NewSkillRevision, SkillApprovalStatus, SkillInstallStatus, SkillLayerRecord,
    SkillRevisionStatus, SkillSnapshotStatus, SkillStateStore,
};
use crate::storage::Storage;
use serde_json::json;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;
use tempfile::TempDir;
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

async fn raw_pool(url: &str) -> SqlitePool {
    let options = SqliteConnectOptions::from_str(url)
        .unwrap()
        .foreign_keys(true);
    SqlitePoolOptions::new()
        .max_connections(2)
        .connect_with(options)
        .await
        .unwrap()
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
async fn active_snapshot_retry_is_idempotent_through_both_entry_points() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    state
        .record_snapshot_candidate(1, json!(["one"]))
        .await
        .unwrap();
    state.record_snapshot_activation(1).await.unwrap();
    state
        .record_snapshot_candidate(2, json!(["two"]))
        .await
        .unwrap();
    state.mark_snapshot_active(2).await.unwrap();
    let lkg_before = state.get_snapshot(1).await.unwrap().unwrap();
    let active_before = state.get_snapshot(2).await.unwrap().unwrap();
    assert_eq!(lkg_before.status, SkillSnapshotStatus::LastKnownGood);
    assert_eq!(active_before.status, SkillSnapshotStatus::Active);

    state.record_snapshot_activation(2).await.unwrap();
    state.mark_snapshot_active(2).await.unwrap();

    assert_eq!(state.get_snapshot(1).await.unwrap().unwrap(), lkg_before);
    assert_eq!(state.get_snapshot(2).await.unwrap().unwrap(), active_before);
}

#[tokio::test]
async fn snapshot_activation_failure_rolls_back_all_status_changes() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    state
        .record_snapshot_candidate(1, json!(["one"]))
        .await
        .unwrap();
    state.record_snapshot_activation(1).await.unwrap();
    state
        .record_snapshot_candidate(2, json!(["two"]))
        .await
        .unwrap();
    sqlx::query(
        r#"CREATE TRIGGER fail_second_snapshot_activation
           BEFORE UPDATE OF status ON skill_snapshots
           WHEN NEW.generation = 2 AND NEW.status = 'active'
           BEGIN
             SELECT RAISE(ABORT, 'snapshot activation failed');
           END"#,
    )
    .execute(storage.pool())
    .await
    .unwrap();

    assert!(state.record_snapshot_activation(2).await.is_err());

    assert_eq!(
        state.get_snapshot(1).await.unwrap().unwrap().status,
        SkillSnapshotStatus::Active
    );
    assert_eq!(
        state.get_snapshot(2).await.unwrap().unwrap().status,
        SkillSnapshotStatus::Candidate
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_approval_resolution_has_one_winner_and_business_loser() {
    let (_directory, url) = file_database();
    let first_storage = Storage::connect(&url).await.unwrap();
    let second_storage = Storage::connect(&url).await.unwrap();
    let first = SkillStateStore::new(first_storage);
    let second = SkillStateStore::new(second_storage);
    let approval = first
        .create_approval(approval_input(package_id("com.example.calendar")))
        .await
        .unwrap();
    let approval_id = approval.approval_id.clone();
    let second_id = approval.approval_id.clone();

    let (approve, reject) = tokio::join!(
        first.approve(&approval_id, "owner-2"),
        second.reject(&second_id, "owner-3")
    );

    assert_eq!(approve.is_ok() as u8 + reject.is_ok() as u8, 1);
    let loser = approve.err().or_else(|| reject.err()).unwrap().to_string();
    assert!(
        loser.contains("already resolved"),
        "unexpected loser error: {loser}"
    );
    assert!(!loser.contains("database is locked"));
    let resolved = first
        .get_approval(&approval.approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(resolved.status, SkillApprovalStatus::Pending);
}

#[tokio::test]
async fn upgrades_task6_schema_with_lifecycle_composite_fk_and_active_check() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_task6_tables(&pool).await;
    let revision_id = Uuid::new_v4().to_string();
    insert_legacy_revision(&pool, &revision_id).await;
    insert_legacy_active_installation(&pool, &revision_id).await;
    pool.close().await;

    let storage = Storage::connect(&url).await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let revision = state.get_revision(&revision_id).await.unwrap().unwrap();
    assert_eq!(revision.status, SkillRevisionStatus::Managed);

    let foreign_keys = sqlx::query("PRAGMA foreign_key_list(skill_installations)")
        .fetch_all(storage.pool())
        .await
        .unwrap();
    assert_eq!(foreign_keys.len(), 2);
    let active_without_revision = sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('com.example.missing', 'managed', NULL, 1, 'approved', 'active', ?, ?)"#,
    )
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(storage.pool())
    .await;
    assert!(active_without_revision.is_err());
    let cross_package_revision = sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('com.example.mail', 'managed', ?, 1, 'approved', 'active', ?, ?)"#,
    )
    .bind(&revision_id)
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(storage.pool())
    .await;
    assert!(cross_package_revision.is_err());

    crate::skill_state::migrate(storage.pool()).await.unwrap();
    crate::skill_state::migrate(storage.pool()).await.unwrap();
}

#[tokio::test]
async fn failed_legacy_schema_upgrade_rolls_back_all_skill_ddl() {
    let (_directory, url) = file_database();
    let pool = raw_pool(&url).await;
    create_legacy_task6_tables(&pool).await;
    let revision_id = Uuid::new_v4().to_string();
    insert_legacy_revision(&pool, &revision_id).await;
    sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('com.example.calendar', 'managed', NULL, 1, 'approved', 'active', ?, ?)"#,
    )
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;

    assert!(Storage::connect(&url).await.is_err());

    let pool = raw_pool(&url).await;
    let lifecycle_column: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM pragma_table_info('skill_revisions') WHERE name = 'lifecycle_status'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(lifecycle_column, 0);
    let legacy_installation: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_installations")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(legacy_installation, 1);
}

#[tokio::test]
async fn public_reads_reject_corrupt_ids_packages_bools_json_and_timestamps() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let now = "2026-01-01T00:00:00Z";
    let valid_revision = Uuid::new_v4().to_string();
    let bad_package_revision = Uuid::new_v4().to_string();
    sqlx::query(
        r#"INSERT INTO skill_revisions
           (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
            validation_json, created_by, created_at, lifecycle_status)
           VALUES ('bad-revision', 'com.example.calendar', '1.0.0', 'h1', 'p1', '{}', '{}', 'o', ?, 'managed')"#,
    )
    .bind(now)
    .execute(storage.pool())
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO skill_revisions
           (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
            validation_json, created_by, created_at, lifecycle_status)
           VALUES (?, 'Bad.package.id', '1.0.0', 'h2', 'p2', '{}', '{}', 'o', ?, 'managed')"#,
    )
    .bind(&bad_package_revision)
    .bind(now)
    .execute(storage.pool())
    .await
    .unwrap();
    assert!(state.get_revision("bad-revision").await.is_err());
    assert!(state.get_revision(&bad_package_revision).await.is_err());

    let mut connection = storage.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('com.example.badbool', 'managed', NULL, 2, 'approved', 'inactive', ?, ?)"#,
    )
    .bind(now)
    .bind(now)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('com.example.badrevision', 'managed', 'not-a-uuid', 1, 'approved', 'active', ?, ?)"#,
    )
    .bind(now)
    .bind(now)
    .execute(&mut *connection)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('Bad.package.id', 'managed', ?, 1, 'approved', 'active', ?, ?)"#,
    )
    .bind(&valid_revision)
    .bind(now)
    .bind(now)
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);
    assert!(
        state
            .get_installation(&package_id("com.example.badbool"))
            .await
            .is_err()
    );
    assert!(
        state
            .get_installation(&package_id("com.example.badrevision"))
            .await
            .is_err()
    );
    assert!(state.list_active_installations().await.is_err());

    insert_corrupt_approval_rows(&storage, now).await;
    assert!(state.get_approval("bad-approval").await.is_err());
    assert!(
        state
            .get_approval("11111111-1111-4111-8111-111111111111")
            .await
            .is_err()
    );
    assert!(
        state
            .get_approval("22222222-2222-4222-8222-222222222222")
            .await
            .is_err()
    );

    insert_corrupt_audit_rows(&storage, now).await;
    assert!(
        state
            .list_audit(&package_id("com.example.auditid"))
            .await
            .is_err()
    );
    assert!(
        state
            .list_audit(&package_id("com.example.auditjson"))
            .await
            .is_err()
    );
    assert!(
        state
            .list_audit(&package_id("com.example.audittime"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn snapshot_and_circuit_reads_are_typed_and_reject_corruption() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let now = "2026-01-01T00:00:00Z";
    state
        .record_snapshot_candidate(1, json!(["rev-1"]))
        .await
        .unwrap();
    let snapshot = state.get_snapshot(1).await.unwrap().unwrap();
    assert_eq!(snapshot.generation, 1);
    assert_eq!(snapshot.members_json, json!(["rev-1"]));

    let mut connection = storage.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    for (generation, status, members, created_at, activated_at) in [
        (2_i64, "future", "[]", now, None),
        (3, "candidate", "{bad", now, None),
        (4, "candidate", "[]", "not-a-time", None),
        (5, "active", "[]", now, Some("not-a-time")),
    ] {
        sqlx::query(
            r#"INSERT INTO skill_snapshots
               (generation, status, members_json, created_at, activated_at)
               VALUES (?, ?, ?, ?, ?)"#,
        )
        .bind(generation)
        .bind(status)
        .bind(members)
        .bind(created_at)
        .bind(activated_at)
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    drop(connection);
    for generation in 2..=5 {
        assert!(state.get_snapshot(generation).await.is_err());
    }

    let revision_id = Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT INTO skill_circuit_state (revision_id, consecutive_failures, open_until, updated_at) VALUES (?, 2, NULL, ?)",
    )
    .bind(&revision_id)
    .bind(now)
    .execute(storage.pool())
    .await
    .unwrap();
    let circuit = state
        .get_circuit_state(&revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(circuit.consecutive_failures, 2);

    let mut connection = storage.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    for (id, failures, open_until, updated_at) in [
        ("bad-revision", 1_i64, None, now),
        ("33333333-3333-4333-8333-333333333333", -1, None, now),
        (
            "44444444-4444-4444-8444-444444444444",
            1,
            Some("not-a-time"),
            now,
        ),
        (
            "55555555-5555-4555-8555-555555555555",
            1,
            None,
            "not-a-time",
        ),
    ] {
        sqlx::query(
            "INSERT INTO skill_circuit_state (revision_id, consecutive_failures, open_until, updated_at) VALUES (?, ?, ?, ?)",
        )
        .bind(id)
        .bind(failures)
        .bind(open_until)
        .bind(updated_at)
        .execute(&mut *connection)
        .await
        .unwrap();
    }
    drop(connection);
    for id in [
        "bad-revision",
        "33333333-3333-4333-8333-333333333333",
        "44444444-4444-4444-8444-444444444444",
        "55555555-5555-4555-8555-555555555555",
    ] {
        assert!(state.get_circuit_state(id).await.is_err());
    }
}

async fn create_legacy_task6_tables(pool: &SqlitePool) {
    sqlx::query(
        r#"CREATE TABLE skill_revisions (
          revision_id TEXT PRIMARY KEY,
          package_id TEXT NOT NULL,
          version TEXT NOT NULL,
          content_hash TEXT NOT NULL,
          storage_path TEXT NOT NULL,
          descriptor_json TEXT NOT NULL,
          validation_json TEXT NOT NULL,
          created_by TEXT NOT NULL,
          created_at TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        r#"CREATE TABLE skill_installations (
          package_id TEXT PRIMARY KEY,
          source_layer TEXT NOT NULL CHECK(source_layer IN ('builtin', 'managed', 'session')),
          active_revision_id TEXT,
          enabled INTEGER NOT NULL CHECK(enabled IN (0, 1)),
          trust_level TEXT NOT NULL CHECK(length(trust_level) > 0),
          install_status TEXT NOT NULL CHECK(install_status IN ('active', 'disabled', 'inactive', 'quarantined', 'removed')),
          installed_at TEXT NOT NULL,
          updated_at TEXT NOT NULL
        )"#,
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE INDEX idx_skill_revisions_package ON skill_revisions(package_id, created_at)",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "CREATE INDEX idx_skill_installations_active ON skill_installations(enabled, install_status)",
    )
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_legacy_revision(pool: &SqlitePool, revision_id: &str) {
    sqlx::query(
        r#"INSERT INTO skill_revisions
           (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
            validation_json, created_by, created_at)
           VALUES (?, 'com.example.calendar', '1.0.0', 'hash', 'managed/revision',
                   '{}', '{}', 'owner-1', '2026-01-01T00:00:00Z')"#,
    )
    .bind(revision_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_legacy_active_installation(pool: &SqlitePool, revision_id: &str) {
    sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level,
            install_status, installed_at, updated_at)
           VALUES ('com.example.calendar', 'managed', ?, 1, 'approved', 'active', ?, ?)"#,
    )
    .bind(revision_id)
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(pool)
    .await
    .unwrap();
}

async fn insert_corrupt_approval_rows(storage: &Storage, now: &str) {
    for (id, permission_diff, resolved_at) in [
        ("bad-approval", "[]", None),
        ("11111111-1111-4111-8111-111111111111", "{bad", None),
        (
            "22222222-2222-4222-8222-222222222222",
            "[]",
            Some("not-a-time"),
        ),
    ] {
        sqlx::query(
            r#"INSERT INTO skill_approvals
               (approval_id, package_id, revision_id, operation, requested_by, approved_by,
                status, permission_diff, created_at, resolved_at)
               VALUES (?, 'com.example.calendar', 'rev-1', 'activate', 'owner-1',
                       'owner-2', 'approved', ?, ?, ?)"#,
        )
        .bind(id)
        .bind(permission_diff)
        .bind(now)
        .bind(resolved_at)
        .execute(storage.pool())
        .await
        .unwrap();
    }
}

async fn insert_corrupt_audit_rows(storage: &Storage, now: &str) {
    for (id, package_id, metadata, created_at) in [
        ("bad-audit", "com.example.auditid", "{}", now),
        (
            "66666666-6666-4666-8666-666666666666",
            "com.example.auditjson",
            "{bad",
            now,
        ),
        (
            "77777777-7777-4777-8777-777777777777",
            "com.example.audittime",
            "{}",
            "not-a-time",
        ),
    ] {
        sqlx::query(
            r#"INSERT INTO skill_audit_log
               (id, actor_id, operation, package_id, revision_id, result, metadata_json, created_at)
               VALUES (?, 'owner-1', 'test', ?, NULL, 'ok', ?, ?)"#,
        )
        .bind(id)
        .bind(package_id)
        .bind(metadata)
        .bind(created_at)
        .execute(storage.pool())
        .await
        .unwrap();
    }
}
