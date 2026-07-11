use crate::skill_package::SkillPackageId;
use crate::skill_state::{
    NewSkillApproval, NewSkillRevision, SkillApprovalStatus, SkillInstallStatus, SkillLayerRecord,
    SkillSnapshotStatus, SkillStateStore,
};
use crate::storage::Storage;
use serde_json::json;
use sqlx::Row;
use uuid::Version;

fn package_id(value: &str) -> SkillPackageId {
    SkillPackageId::parse(value).unwrap()
}

fn new_revision(package_id: SkillPackageId) -> NewSkillRevision {
    NewSkillRevision {
        package_id,
        version: "1.0.0".into(),
        content_hash: "abc123".into(),
        storage_path: "managed/com.example.calendar/revisions/rev-1".into(),
        descriptor_json: json!({"schemaVersion": 1}),
        validation_json: json!({"ok": true}),
        created_by: "owner-1".into(),
    }
}

fn new_approval(package_id: SkillPackageId, requested_by: &str) -> NewSkillApproval {
    NewSkillApproval {
        package_id,
        revision_id: "rev-1".into(),
        operation: "activate".into(),
        requested_by: requested_by.into(),
        permission_diff: json!([]),
    }
}

#[tokio::test]
async fn persists_revision_activation_and_audit_atomically() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let package_id = package_id("com.example.calendar");
    let revision = state
        .create_revision(new_revision(package_id.clone()))
        .await
        .unwrap();
    assert_eq!(
        uuid::Uuid::parse_str(&revision.revision_id)
            .unwrap()
            .get_version(),
        Some(Version::Random)
    );

    state
        .activate_revision(
            &package_id,
            &revision.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();

    let installation = state.get_installation(&package_id).await.unwrap().unwrap();
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(revision.revision_id.as_str())
    );
    assert_eq!(installation.status, SkillInstallStatus::Active);
    assert!(installation.enabled);
    let audit = state.list_audit(&package_id).await.unwrap();
    assert_eq!(audit.last().unwrap().operation, "activate_revision");
    assert_eq!(
        uuid::Uuid::parse_str(&audit.last().unwrap().id)
            .unwrap()
            .get_version(),
        Some(Version::Random)
    );
}

#[tokio::test]
async fn activation_rolls_back_installation_when_audit_insert_fails() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let package_id = package_id("com.example.calendar");
    let revision = state
        .create_revision(new_revision(package_id.clone()))
        .await
        .unwrap();
    sqlx::query(
        r#"
        CREATE TRIGGER fail_activation_audit
        BEFORE INSERT ON skill_audit_log
        WHEN NEW.operation = 'activate_revision'
        BEGIN
            SELECT RAISE(ABORT, 'audit failed');
        END;
        "#,
    )
    .execute(storage.pool())
    .await
    .unwrap();

    let result = state
        .activate_revision(
            &package_id,
            &revision.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await;

    assert!(result.is_err());
    assert!(state.get_installation(&package_id).await.unwrap().is_none());
    assert!(state.list_audit(&package_id).await.unwrap().is_empty());
}

#[tokio::test]
async fn activation_rejects_missing_or_wrong_package_revision_without_writes() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let calendar = package_id("com.example.calendar");
    let mail = package_id("com.example.mail");
    let revision = state
        .create_revision(new_revision(calendar.clone()))
        .await
        .unwrap();

    assert!(
        state
            .activate_revision(
                &calendar,
                "missing-revision",
                SkillLayerRecord::Managed,
                "owner-1",
            )
            .await
            .is_err()
    );
    assert!(
        state
            .activate_revision(
                &mail,
                &revision.revision_id,
                SkillLayerRecord::Managed,
                "owner-1",
            )
            .await
            .is_err()
    );

    assert!(state.get_installation(&calendar).await.unwrap().is_none());
    assert!(state.get_installation(&mail).await.unwrap().is_none());
    assert!(state.list_audit(&calendar).await.unwrap().is_empty());
    assert!(state.list_audit(&mail).await.unwrap().is_empty());
}

#[tokio::test]
async fn revision_round_trip_and_validation_update_are_typed() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let package_id = package_id("com.example.calendar");
    let created = state
        .create_revision(new_revision(package_id.clone()))
        .await
        .unwrap();

    let loaded = state
        .get_revision(&created.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.package_id, package_id);
    assert_eq!(loaded.descriptor_json, json!({"schemaVersion": 1}));
    assert_eq!(
        state
            .revision_validation(&created.revision_id)
            .await
            .unwrap(),
        json!({"ok": true})
    );

    state
        .update_revision_validation(
            &created.revision_id,
            json!({"ok": false, "errors": ["bad"]}),
        )
        .await
        .unwrap();
    assert_eq!(
        state
            .revision_validation(&created.revision_id)
            .await
            .unwrap(),
        json!({"ok": false, "errors": ["bad"]})
    );
    assert!(
        state
            .update_revision_validation("missing-revision", json!({}))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn approval_cannot_be_approved_by_requesting_actor() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let approval = state
        .create_approval(new_approval(package_id("com.example.calendar"), "owner-1"))
        .await
        .unwrap();
    assert_eq!(
        uuid::Uuid::parse_str(&approval.approval_id)
            .unwrap()
            .get_version(),
        Some(Version::Random)
    );

    let error = state
        .approve(&approval.approval_id, "owner-1")
        .await
        .unwrap_err();

    assert!(error.to_string().contains("requester cannot approve"));
    let loaded = state
        .get_approval(&approval.approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(loaded.status, SkillApprovalStatus::Pending);
    assert!(loaded.approved_by.is_none());
    assert!(loaded.resolved_at.is_none());
}

#[tokio::test]
async fn resolved_approval_cannot_be_approved_or_rejected_again() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let approval = state
        .create_approval(new_approval(package_id("com.example.calendar"), "owner-1"))
        .await
        .unwrap();
    let approved = state
        .approve(&approval.approval_id, "owner-2")
        .await
        .unwrap();

    assert_eq!(approved.status, SkillApprovalStatus::Approved);
    assert_eq!(approved.approved_by.as_deref(), Some("owner-2"));
    assert!(approved.resolved_at.is_some());
    assert!(
        state
            .approve(&approval.approval_id, "owner-3")
            .await
            .is_err()
    );
    assert!(
        state
            .reject(&approval.approval_id, "owner-3")
            .await
            .is_err()
    );

    let unchanged = state
        .get_approval(&approval.approval_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.status, SkillApprovalStatus::Approved);
    assert_eq!(unchanged.approved_by.as_deref(), Some("owner-2"));
    assert_eq!(unchanged.resolved_at, approved.resolved_at);
}

#[tokio::test]
async fn pending_approval_can_be_rejected_once() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let approval = state
        .create_approval(new_approval(package_id("com.example.calendar"), "owner-1"))
        .await
        .unwrap();

    let rejected = state
        .reject(&approval.approval_id, "owner-2")
        .await
        .unwrap();

    assert_eq!(rejected.status, SkillApprovalStatus::Rejected);
    assert_eq!(rejected.approved_by.as_deref(), Some("owner-2"));
    assert!(rejected.resolved_at.is_some());
    assert!(
        state
            .reject(&approval.approval_id, "owner-3")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn row_conversion_rejects_unknown_enum_values() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let mut connection = storage.pool().acquire().await.unwrap();
    sqlx::query("PRAGMA ignore_check_constraints = ON")
        .execute(&mut *connection)
        .await
        .unwrap();
    sqlx::query(
        r#"INSERT INTO skill_installations
           (package_id, source_layer, active_revision_id, enabled, trust_level, install_status, installed_at, updated_at)
           VALUES (?, 'future', NULL, 1, 'approved', 'active', ?, ?)"#,
    )
    .bind("com.example.calendar")
    .bind("2026-01-01T00:00:00Z")
    .bind("2026-01-01T00:00:00Z")
    .execute(&mut *connection)
    .await
    .unwrap();
    drop(connection);

    let error = state
        .get_installation(&package_id("com.example.calendar"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("unknown skill layer"));
}

#[tokio::test]
async fn row_conversion_rejects_bad_json_and_bad_timestamps() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    sqlx::query(
        r#"INSERT INTO skill_revisions
           (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
            validation_json, created_by, created_at)
           VALUES ('bad-json', 'com.example.calendar', '1.0.0', 'hash-1', 'path-1',
                   '{not-json', '{}', 'owner-1', '2026-01-01T00:00:00Z')"#,
    )
    .execute(storage.pool())
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO skill_revisions
           (revision_id, package_id, version, content_hash, storage_path, descriptor_json,
            validation_json, created_by, created_at)
           VALUES ('bad-time', 'com.example.calendar', '1.0.1', 'hash-2', 'path-2',
                   '{}', '{}', 'owner-1', 'not-a-time')"#,
    )
    .execute(storage.pool())
    .await
    .unwrap();

    assert!(state.get_revision("bad-json").await.is_err());
    assert!(state.get_revision("bad-time").await.is_err());
}

#[tokio::test]
async fn snapshot_generation_overflow_does_not_write() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());

    let error = state
        .record_snapshot_candidate(u64::MAX, json!(["rev-1"]))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("generation"));
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_snapshots")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn snapshot_state_transitions_require_existing_generation() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());

    assert!(state.mark_snapshot_active(7).await.is_err());
    assert!(state.mark_snapshot_last_known_good(7).await.is_err());

    state
        .record_snapshot_candidate(7, json!(["rev-1"]))
        .await
        .unwrap();
    state.mark_snapshot_active(7).await.unwrap();
    let active = snapshot_status(&storage, 7).await;
    assert_eq!(active, SkillSnapshotStatus::Active.as_str());

    state.mark_snapshot_last_known_good(7).await.unwrap();
    let last_known_good = snapshot_status(&storage, 7).await;
    assert_eq!(last_known_good, SkillSnapshotStatus::LastKnownGood.as_str());
}

#[tokio::test]
async fn quarantining_revision_disables_only_matching_installation_and_records_reason() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let calendar = package_id("com.example.calendar");
    let mail = package_id("com.example.mail");
    let calendar_revision = state
        .create_revision(new_revision(calendar.clone()))
        .await
        .unwrap();
    let mut mail_input = new_revision(mail.clone());
    mail_input.content_hash = "mail-hash".into();
    mail_input.storage_path = "managed/com.example.mail/revisions/rev-1".into();
    let mail_revision = state.create_revision(mail_input).await.unwrap();
    state
        .activate_revision(
            &calendar,
            &calendar_revision.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();
    state
        .activate_revision(
            &mail,
            &mail_revision.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();

    state
        .mark_revision_quarantined(&calendar_revision.revision_id, "signature revoked")
        .await
        .unwrap();

    let calendar_installation = state.get_installation(&calendar).await.unwrap().unwrap();
    assert_eq!(
        calendar_installation.status,
        SkillInstallStatus::Quarantined
    );
    assert!(!calendar_installation.enabled);
    assert!(calendar_installation.active_revision_id.is_none());
    let mail_installation = state.get_installation(&mail).await.unwrap().unwrap();
    assert_eq!(mail_installation.status, SkillInstallStatus::Active);
    assert!(mail_installation.enabled);
    assert_eq!(
        state
            .revision_validation(&calendar_revision.revision_id)
            .await
            .unwrap()["quarantineReason"],
        "signature revoked"
    );
    let audit = state.list_audit(&calendar).await.unwrap();
    let quarantine = audit
        .iter()
        .find(|entry| entry.operation == "mark_revision_quarantined")
        .unwrap();
    assert_eq!(quarantine.metadata_json["reason"], "signature revoked");
    assert!(
        state
            .mark_revision_quarantined("missing-revision", "bad")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn active_installation_listing_excludes_quarantined_entries() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage);
    let calendar = package_id("com.example.calendar");
    let revision = state
        .create_revision(new_revision(calendar.clone()))
        .await
        .unwrap();
    state
        .activate_revision(
            &calendar,
            &revision.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();
    assert_eq!(state.list_active_installations().await.unwrap().len(), 1);

    state
        .mark_revision_quarantined(&revision.revision_id, "bad")
        .await
        .unwrap();

    assert!(state.list_active_installations().await.unwrap().is_empty());
}

#[tokio::test]
async fn skill_state_migration_is_idempotent_with_foreign_keys_enabled() {
    let storage = Storage::connect("sqlite::memory:").await.unwrap();

    crate::skill_state::migrate(storage.pool()).await.unwrap();
    crate::skill_state::migrate(storage.pool()).await.unwrap();

    let foreign_keys: i64 = sqlx::query_scalar("PRAGMA foreign_keys")
        .fetch_one(storage.pool())
        .await
        .unwrap();
    assert_eq!(foreign_keys, 1);
    let rows =
        sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table' AND name LIKE 'skill_%'")
            .fetch_all(storage.pool())
            .await
            .unwrap();
    assert_eq!(rows.len(), 6);
}

async fn snapshot_status(storage: &Storage, generation: u64) -> String {
    let generation = i64::try_from(generation).unwrap();
    sqlx::query("SELECT status FROM skill_snapshots WHERE generation = ?")
        .bind(generation)
        .fetch_one(storage.pool())
        .await
        .unwrap()
        .try_get("status")
        .unwrap()
}
