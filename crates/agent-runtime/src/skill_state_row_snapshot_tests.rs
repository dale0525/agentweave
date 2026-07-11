use crate::skill_package::SkillPackageId;
use crate::skill_state::{SkillSnapshotStatus, SkillStateStore};
use crate::storage::Storage;
use serde_json::json;
use uuid::Uuid;

fn package_id(value: &str) -> SkillPackageId {
    SkillPackageId::parse(value).unwrap()
}

#[tokio::test]
async fn snapshot_active_and_last_known_good_sequence_is_stable_and_idempotent() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    state
        .record_snapshot_candidate(1, json!(["one"]))
        .await
        .unwrap();
    state.record_snapshot_activation(1).await.unwrap();
    let active_one = state.get_snapshot(1).await.unwrap().unwrap();
    state.record_snapshot_activation(1).await.unwrap();
    assert_eq!(state.get_snapshot(1).await.unwrap().unwrap(), active_one);

    state.mark_snapshot_last_known_good(1).await.unwrap();
    let lkg_one = state.get_snapshot(1).await.unwrap().unwrap();
    assert_eq!(lkg_one.status, SkillSnapshotStatus::LastKnownGood);
    state.mark_snapshot_last_known_good(1).await.unwrap();
    state.record_snapshot_activation(1).await.unwrap();
    assert_eq!(state.get_snapshot(1).await.unwrap().unwrap(), lkg_one);

    state
        .record_snapshot_candidate(2, json!(["two"]))
        .await
        .unwrap();
    state.mark_snapshot_active(2).await.unwrap();
    let active_two = state.get_snapshot(2).await.unwrap().unwrap();
    assert_eq!(active_two.status, SkillSnapshotStatus::Active);
    assert_eq!(state.get_snapshot(1).await.unwrap().unwrap(), lkg_one);
    state.record_snapshot_activation(2).await.unwrap();
    assert_eq!(state.get_snapshot(2).await.unwrap().unwrap(), active_two);
    assert_eq!(state.get_snapshot(1).await.unwrap().unwrap(), lkg_one);

    state.mark_snapshot_last_known_good(2).await.unwrap();
    let lkg_two = state.get_snapshot(2).await.unwrap().unwrap();
    assert_eq!(lkg_two.status, SkillSnapshotStatus::LastKnownGood);
    assert_eq!(
        state.get_snapshot(1).await.unwrap().unwrap().status,
        SkillSnapshotStatus::Candidate
    );
    state.mark_snapshot_last_known_good(2).await.unwrap();
    state.record_snapshot_activation(2).await.unwrap();
    assert_eq!(state.get_snapshot(2).await.unwrap().unwrap(), lkg_two);
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
