use crate::skill_authoring_tests::{AuthoringFixture, update};
use crate::skill_management::SkillManagementError;
use crate::skill_policy::{ActorContext, SkillGrant};
use crate::skill_recovery::RecoveryStatus;
use crate::skill_state::{SkillApprovalStatus, SkillInstallStatus};
use crate::tools::ToolSource;
use chrono::{Duration, Utc};
use serde_json::json;
use std::sync::Arc;

pub(crate) async fn activate_new_revision(fixture: &AuthoringFixture, version: &str) -> String {
    let draft = fixture.draft().await;
    let record = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    let mut descriptor = record.descriptor_json;
    descriptor["version"] = json!(version);
    fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![update(
                "general-agent.json",
                format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
            )],
        )
        .await
        .unwrap();
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    fixture
        .service
        .approve_activation(
            &approval.approval_id,
            &ActorContext::owner("approver-2", [SkillGrant::Activate]),
        )
        .await
        .unwrap();
    draft.revision_id
}

fn source(package_id: &str, revision_id: &str) -> ToolSource {
    ToolSource::RuntimeSkill {
        skill_name: "calendar-runtime".into(),
        package_id: package_id.into(),
        revision_id: Some(revision_id.into()),
    }
}

#[tokio::test]
async fn lease_owns_the_exact_turn_snapshot_after_publication() {
    let fixture = AuthoringFixture::new().await;
    let lease = fixture.manager.lease_snapshot();
    let captured = lease.snapshot_arc();

    activate_new_revision(&fixture, "1.0.0").await;

    assert_eq!(lease.generation(), 1);
    assert!(Arc::ptr_eq(&captured, &lease.snapshot_arc()));
    assert!(!Arc::ptr_eq(
        &lease.snapshot_arc(),
        &fixture.manager.current_snapshot()
    ));
}

#[tokio::test]
async fn rollback_publishes_previous_revision_for_later_leases_only() {
    let fixture = AuthoringFixture::new().await;
    let first = activate_new_revision(&fixture, "1.0.0").await;
    let old_turn = fixture.manager.lease_snapshot();
    let second = activate_new_revision(&fixture, "2.0.0").await;

    let outcome = fixture
        .service
        .rollback_managed_skill(
            &fixture.actor([SkillGrant::Rollback]),
            &crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap(),
            &first,
        )
        .await
        .unwrap();
    let crate::skill_management::SkillRollbackOutcome::Published(report) = outcome else {
        panic!("default rollback policy must publish immediately")
    };

    assert_eq!(report.active_revision_id, first);
    assert_eq!(report.replaced_revision_id, second);
    assert_eq!(old_turn.generation(), 2);
    assert_eq!(fixture.manager.lease_snapshot().generation(), 4);
}

#[tokio::test]
async fn disable_keeps_the_running_lease_and_hides_the_package_next_turn() {
    let fixture = AuthoringFixture::new().await;
    activate_new_revision(&fixture, "1.0.0").await;
    let running = fixture.manager.lease_snapshot();

    fixture
        .service
        .disable_managed_skill(
            &fixture.actor([SkillGrant::Disable]),
            &crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(running.snapshot().packages().len(), 1);
    assert!(
        fixture
            .manager
            .lease_snapshot()
            .snapshot()
            .packages()
            .is_empty()
    );
    let installation = fixture
        .state
        .get_installation(
            &crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(installation.status, SkillInstallStatus::Disabled);
}

#[tokio::test]
async fn removal_requires_a_different_actor_and_is_single_use() {
    let fixture = AuthoringFixture::new().await;
    activate_new_revision(&fixture, "1.0.0").await;
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    let requester = fixture.actor([SkillGrant::DeleteManaged]);
    let approval = fixture
        .service
        .request_removal(&requester, &package_id)
        .await
        .unwrap();

    assert_eq!(approval.status, SkillApprovalStatus::Pending);
    let own_error = fixture
        .service
        .approve_removal(&approval.approval_id, &requester)
        .await
        .unwrap_err();
    assert!(matches!(
        own_error.downcast_ref::<SkillManagementError>(),
        Some(SkillManagementError::Conflict { .. })
    ));
    fixture
        .service
        .approve_removal(
            &approval.approval_id,
            &ActorContext::owner("approver-2", [SkillGrant::DeleteManaged]),
        )
        .await
        .unwrap();
    let duplicate = fixture
        .service
        .approve_removal(
            &approval.approval_id,
            &ActorContext::owner("approver-2", [SkillGrant::DeleteManaged]),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        duplicate.downcast_ref::<SkillManagementError>(),
        Some(SkillManagementError::Conflict { .. })
    ));
}

#[tokio::test]
async fn disabled_managed_installation_can_be_removed_with_distinct_approval() {
    let fixture = AuthoringFixture::new().await;
    activate_new_revision(&fixture, "1.0.0").await;
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    fixture
        .service
        .disable_managed_skill(&fixture.actor([SkillGrant::Disable]), &package_id)
        .await
        .unwrap();

    let approval = fixture
        .service
        .request_removal(&fixture.actor([SkillGrant::DeleteManaged]), &package_id)
        .await
        .unwrap();
    fixture
        .service
        .approve_removal(
            &approval.approval_id,
            &ActorContext::owner("disabled-approver", [SkillGrant::DeleteManaged]),
        )
        .await
        .unwrap();

    let installation = fixture
        .state
        .get_installation(&package_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(installation.status, SkillInstallStatus::Removed);
    assert!(!installation.enabled);
    assert!(fixture.manager.current_snapshot().packages().is_empty());
}

#[tokio::test]
async fn circuit_opens_on_three_failures_and_success_resets_the_sequence() {
    let fixture = AuthoringFixture::new().await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let source = source("com.example.calendar", &revision);

    fixture
        .manager
        .record_execution_result(&source, false)
        .await
        .unwrap();
    fixture
        .manager
        .record_execution_result(&source, true)
        .await
        .unwrap();
    for _ in 0..2 {
        fixture
            .manager
            .record_execution_result(&source, false)
            .await
            .unwrap();
    }
    let closed = fixture
        .state
        .get_circuit_state(&revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(closed.consecutive_failures, 2);
    assert!(closed.open_until.is_none());

    fixture
        .manager
        .record_execution_result(&source, false)
        .await
        .unwrap();
    let open = fixture
        .state
        .get_circuit_state(&revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(open.consecutive_failures, 3);
    let remaining = open.open_until.unwrap() - open.updated_at;
    assert_eq!(remaining, Duration::minutes(5));
}

#[tokio::test]
async fn circuit_ignores_sources_without_a_managed_revision_and_reloads_after_expiry() {
    let fixture = AuthoringFixture::new().await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let unmanaged = ToolSource::RuntimeSkill {
        skill_name: "development".into(),
        package_id: "com.example.development".into(),
        revision_id: None,
    };
    for _ in 0..3 {
        fixture
            .manager
            .record_execution_result(&unmanaged, false)
            .await
            .unwrap();
    }
    assert!(
        fixture
            .state
            .get_circuit_state(&revision)
            .await
            .unwrap()
            .is_none()
    );

    for _ in 0..3 {
        fixture
            .manager
            .record_execution_result(&source("com.example.calendar", &revision), false)
            .await
            .unwrap();
    }
    fixture.manager.reload().await.unwrap();
    assert!(fixture.manager.current_snapshot().packages().is_empty());
    assert!(
        fixture
            .manager
            .current_snapshot()
            .inactive()
            .iter()
            .any(|item| {
                item.package.descriptor.id.as_str() == "com.example.calendar"
                    && item.reason.contains("circuit open")
            })
    );

    sqlx::query("UPDATE skill_circuit_state SET open_until = ? WHERE revision_id = ?")
        .bind((Utc::now() - Duration::seconds(1)).to_rfc3339())
        .bind(&revision)
        .execute(fixture.state.pool())
        .await
        .unwrap();
    fixture.manager.reload().await.unwrap();
    assert_eq!(fixture.manager.current_snapshot().packages().len(), 1);
}

#[tokio::test]
async fn corrupted_active_revision_restores_verified_last_known_good_idempotently() {
    let fixture = AuthoringFixture::new().await;
    let first = activate_new_revision(&fixture, "1.0.0").await;
    let second = activate_new_revision(&fixture, "2.0.0").await;
    let record = fixture.state.get_revision(&second).await.unwrap().unwrap();
    let descriptor = std::path::Path::new(&record.storage_path).join("general-agent.json");
    make_file_writable(&descriptor).await;
    tokio::fs::write(&descriptor, b"corrupt").await.unwrap();

    let restored = fixture.manager.startup_reconcile().await.unwrap();
    let quarantine_entries_after_restore =
        directory_entry_count(fixture.store.paths().quarantine.clone()).await;
    let quarantine_rows_after_restore: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_revisions WHERE lifecycle_status = 'quarantined'",
    )
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    let repeated = fixture.manager.startup_reconcile().await.unwrap();

    assert_eq!(restored.status, RecoveryStatus::LastKnownGoodRestored);
    assert_eq!(restored.generation, 2);
    assert_eq!(repeated.status, RecoveryStatus::CurrentSnapshotValid);
    assert_eq!(repeated.generation, 2);
    assert_eq!(
        directory_entry_count(fixture.store.paths().quarantine.clone()).await,
        quarantine_entries_after_restore
    );
    let quarantine_rows_after_repeat: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_revisions WHERE lifecycle_status = 'quarantined'",
    )
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(quarantine_rows_after_repeat, quarantine_rows_after_restore);
    let installation = fixture
        .state
        .get_installation(
            &crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(first.as_str())
    );
    let audits: Vec<String> = sqlx::query_scalar(
        r#"SELECT metadata_json FROM skill_audit_log
           WHERE operation = 'restore_last_known_good' AND package_id = ? AND revision_id = ?"#,
    )
    .bind("com.example.calendar")
    .bind(&first)
    .fetch_all(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(audits.len(), 1);
    assert!(!audits[0].contains(&record.storage_path));
}

#[tokio::test]
async fn removal_approval_is_stale_after_a_conflicting_publication() {
    let fixture = AuthoringFixture::new().await;
    activate_new_revision(&fixture, "1.0.0").await;
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    let approval = fixture
        .service
        .request_removal(&fixture.actor([SkillGrant::DeleteManaged]), &package_id)
        .await
        .unwrap();
    activate_new_revision(&fixture, "2.0.0").await;

    let error = fixture
        .service
        .approve_removal(
            &approval.approval_id,
            &ActorContext::owner("approver-2", [SkillGrant::DeleteManaged]),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillManagementError>(),
        Some(SkillManagementError::Conflict { .. })
    ));
    assert_eq!(fixture.manager.current_snapshot().generation(), 3);
}

#[tokio::test]
async fn protected_managed_package_denies_destructive_lifecycle_operations() {
    let fixture = AuthoringFixture::new().await;
    activate_new_revision(&fixture, "1.0.0").await;
    activate_new_revision(&fixture, "2.0.0").await;
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    let protected = crate::skill_management::OwnerSkillManagementService::new(
        fixture.manager.clone(),
        fixture.store.clone(),
        fixture.state.clone(),
        crate::skill_policy::SkillManagementPolicy::owner_only().protect(package_id.clone()),
    );

    let rollback = protected
        .rollback_managed_skill(
            &fixture.actor([SkillGrant::Rollback]),
            &package_id,
            &uuid::Uuid::new_v4().to_string(),
        )
        .await
        .unwrap_err();
    let disable = protected
        .disable_managed_skill(&fixture.actor([SkillGrant::Disable]), &package_id)
        .await
        .unwrap_err();
    let removal = protected
        .request_removal(&fixture.actor([SkillGrant::DeleteManaged]), &package_id)
        .await
        .unwrap_err();

    for error in [rollback, disable, removal] {
        assert!(matches!(
            error.downcast_ref::<SkillManagementError>(),
            Some(SkillManagementError::Denied { .. })
        ));
    }
    assert_eq!(fixture.manager.current_snapshot().generation(), 3);
}

#[tokio::test]
async fn builtin_package_without_managed_layer_returns_not_found_for_destructive_operations() {
    let fixture = AuthoringFixture::with_known_runtime_tool().await;
    let package_id =
        crate::skill_package::SkillPackageId::parse("com.example.host-runtime").unwrap();

    let disable = fixture
        .service
        .disable_managed_skill(&fixture.actor([SkillGrant::Disable]), &package_id)
        .await
        .unwrap_err();
    let removal = fixture
        .service
        .request_removal(&fixture.actor([SkillGrant::DeleteManaged]), &package_id)
        .await
        .unwrap_err();

    for error in [disable, removal] {
        assert!(matches!(
            error.downcast_ref::<SkillManagementError>(),
            Some(SkillManagementError::NotFound { .. })
        ));
    }
}

#[tokio::test]
async fn manager_execution_observer_records_the_managed_revision_result() {
    let fixture = AuthoringFixture::new().await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let source = source("com.example.calendar", &revision);

    crate::tools::ToolExecutionObserver::finished(&fixture.manager, &source, false)
        .await
        .unwrap();

    let circuit = fixture
        .state
        .get_circuit_state(&revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(circuit.consecutive_failures, 1);
}

#[tokio::test]
async fn startup_reconcile_emits_distinct_restored_and_current_events() {
    let fixture = AuthoringFixture::new().await;
    activate_new_revision(&fixture, "1.0.0").await;
    let second = activate_new_revision(&fixture, "2.0.0").await;
    let record = fixture.state.get_revision(&second).await.unwrap().unwrap();
    let descriptor = std::path::Path::new(&record.storage_path).join("general-agent.json");
    make_file_writable(&descriptor).await;
    tokio::fs::write(descriptor, b"corrupt").await.unwrap();
    let mut events = fixture.service.subscribe_events();

    fixture.manager.startup_reconcile().await.unwrap();
    fixture.manager.startup_reconcile().await.unwrap();

    assert!(matches!(
        events.recv().await.unwrap(),
        crate::events::RuntimeEvent::SkillRecoveryCompleted {
            status: RecoveryStatus::LastKnownGoodRestored,
            ..
        }
    ));
    assert!(matches!(
        events.recv().await.unwrap(),
        crate::events::RuntimeEvent::SkillRecoveryCompleted {
            status: RecoveryStatus::CurrentSnapshotValid,
            ..
        }
    ));
}

async fn make_file_writable(path: &std::path::Path) {
    let mut permissions = tokio::fs::metadata(path).await.unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    tokio::fs::set_permissions(path, permissions).await.unwrap();
}

async fn directory_entry_count(path: std::path::PathBuf) -> usize {
    let mut entries = tokio::fs::read_dir(path).await.unwrap();
    let mut count = 0;
    while entries.next_entry().await.unwrap().is_some() {
        count += 1;
    }
    count
}
