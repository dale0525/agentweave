use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_management::OwnerSkillManagementService;
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_policy::SkillManagementPolicy;
use crate::skill_recovery::{parse_snapshot_members, snapshot_members};
use crate::skill_recovery_tests::activate_new_revision;
use crate::skill_source::{ManagedSkillSource, SkillSource};
use crate::skill_state::{SkillSnapshotStatus, SkillStateBoundaryError};
use crate::skill_store::{SkillStoreFaultPoint, SkillStoreTestFaults};
use crate::tools::ToolSource;
use chrono::{Duration, Utc};
use std::sync::Arc;

#[tokio::test]
async fn restart_after_circuit_expiry_publishes_the_still_active_installation() {
    let fixture = AuthoringFixture::new().await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let source = managed_source(&revision);
    for _ in 0..3 {
        fixture
            .manager
            .record_execution_result(&source, false)
            .await
            .unwrap();
    }
    let open_generation = fixture.manager.current_snapshot().generation();
    sqlx::query("UPDATE skill_circuit_state SET open_until = ? WHERE revision_id = ?")
        .bind((Utc::now() - Duration::seconds(1)).to_rfc3339())
        .bind(&revision)
        .execute(fixture.state.pool())
        .await
        .unwrap();

    let restarted = manager_for_store(&fixture).await;
    let _service = OwnerSkillManagementService::new(
        restarted.clone(),
        fixture.store.clone(),
        fixture.state.clone(),
        SkillManagementPolicy::owner_only(),
    );
    restarted.startup_reconcile().await.unwrap();

    let active = fixture
        .state
        .snapshot_with_status(SkillSnapshotStatus::Active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.generation, open_generation + 1);
    assert_eq!(restarted.current_snapshot().generation(), active.generation);
    assert_eq!(restarted.current_snapshot().packages().len(), 1);
    assert!(restarted.current_snapshot().inactive().is_empty());
    assert_eq!(
        parse_snapshot_members(active.members_json)
            .unwrap()
            .into_iter()
            .filter_map(|member| member.revision_id)
            .collect::<Vec<_>>(),
        vec![revision]
    );
}

#[tokio::test]
async fn stale_restore_cannot_demote_a_newer_authoritative_snapshot() {
    let fixture = AuthoringFixture::with_faults(SkillStoreTestFaults::default()).await;
    activate_new_revision(&fixture, "1.0.0").await;
    let expected_active = fixture
        .state
        .snapshot_with_status(SkillSnapshotStatus::Active)
        .await
        .unwrap()
        .unwrap();
    let target = fixture
        .state
        .snapshot_with_status(SkillSnapshotStatus::LastKnownGood)
        .await
        .unwrap()
        .unwrap();
    let target_members = parse_snapshot_members(target.members_json.clone()).unwrap();
    let newest = activate_new_revision(&fixture, "2.0.0").await;
    let recovery_connection = fixture.second_state_connection().await;

    let error = recovery_connection
        .restore_snapshot_as_active(&expected_active, &target, &target_members)
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillStateBoundaryError>(),
        Some(SkillStateBoundaryError::Conflict(_))
    ));
    let active = fixture
        .state
        .snapshot_with_status(SkillSnapshotStatus::Active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.generation, expected_active.generation + 1);
    assert!(active.members_json.to_string().contains(&newest));
}

#[tokio::test]
async fn stale_recovery_candidate_cannot_overwrite_a_newer_generation() {
    let fixture = AuthoringFixture::with_faults(SkillStoreTestFaults::default()).await;
    activate_new_revision(&fixture, "1.0.0").await;
    let expected_active = fixture
        .state
        .snapshot_with_status(SkillSnapshotStatus::Active)
        .await
        .unwrap()
        .unwrap();
    let newest = activate_new_revision(&fixture, "2.0.0").await;
    let stale_members = snapshot_members(&fixture.manager.current_snapshot());
    let recovery_connection = fixture.second_state_connection().await;

    let error = recovery_connection
        .persist_recovery_candidate(
            &expected_active,
            expected_active.generation + 1,
            &stale_members,
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillStateBoundaryError>(),
        Some(SkillStateBoundaryError::Conflict(_))
    ));
    let active = fixture
        .state
        .snapshot_with_status(SkillSnapshotStatus::Active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.generation, expected_active.generation + 1);
    assert!(active.members_json.to_string().contains(&newest));
}

async fn manager_for_store(fixture: &AuthoringFixture) -> SkillManager {
    SkillManager::new(SkillManagerConfig {
        sources: vec![
            Arc::new(ManagedSkillSource::from_store(fixture.store.clone())) as Arc<dyn SkillSource>,
        ],
        platform: PlatformId::Server,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
    })
    .await
    .unwrap()
}

fn managed_source(revision_id: &str) -> ToolSource {
    ToolSource::RuntimeSkill {
        skill_name: "calendar-runtime".into(),
        package_id: "com.example.calendar".into(),
        revision_id: Some(revision_id.into()),
    }
}

#[tokio::test]
async fn cancelled_circuit_waiter_finishes_the_exact_publication_once() {
    let faults = SkillStoreTestFaults::default();
    let gate = faults.gate_once(SkillStoreFaultPoint::CircuitAfterDurableCommit);
    let fixture = AuthoringFixture::with_faults(faults).await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let generation = fixture.manager.current_snapshot().generation();
    let source = managed_source(&revision);
    for _ in 0..2 {
        fixture
            .manager
            .record_execution_result(&source, false)
            .await
            .unwrap();
    }
    let manager = fixture.manager.clone();
    let waiter = tokio::spawn(async move { manager.record_execution_result(&source, false).await });

    tokio::time::timeout(std::time::Duration::from_secs(2), gate.wait_entered())
        .await
        .expect("circuit publication must reach the post-commit gate");
    waiter.abort();
    gate.release().await;

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if fixture.manager.current_snapshot().generation() == generation + 1
                && fixture.manager.current_snapshot().packages().is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned circuit finalizer must converge after waiter cancellation");
    let publications: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_audit_log WHERE operation = 'open_skill_revision_circuit'",
    )
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(publications, 1);
    let active_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM skill_snapshots WHERE status = 'active'")
            .fetch_one(fixture.state.pool())
            .await
            .unwrap();
    assert_eq!(active_count, 1);
}

#[tokio::test]
async fn cancelled_after_circuit_row_transition_still_publishes_in_process() {
    let faults = SkillStoreTestFaults::default();
    let gate = faults.gate_once(SkillStoreFaultPoint::CircuitAfterStateTransition);
    let fixture = AuthoringFixture::with_faults(faults).await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let generation = fixture.manager.current_snapshot().generation();
    let source = managed_source(&revision);
    for _ in 0..2 {
        fixture
            .manager
            .record_execution_result(&source, false)
            .await
            .unwrap();
    }
    let manager = fixture.manager.clone();
    let waiter = tokio::spawn(async move { manager.record_execution_result(&source, false).await });

    gate.wait_entered().await;
    waiter.abort();
    gate.release().await;

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if fixture.manager.current_snapshot().generation() == generation + 1
                && fixture.manager.current_snapshot().packages().is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned circuit transition must publish after waiter cancellation");
}

#[tokio::test]
async fn stale_second_manager_rebuilds_the_authoritative_circuit_snapshot() {
    let fixture = AuthoringFixture::new().await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let second = manager_for_store(&fixture).await;
    let _second_service = OwnerSkillManagementService::new(
        second.clone(),
        fixture.store.clone(),
        fixture.state.clone(),
        SkillManagementPolicy::owner_only(),
    );
    second.startup_reconcile().await.unwrap();
    let source = managed_source(&revision);
    for _ in 0..3 {
        fixture
            .manager
            .record_execution_result(&source, false)
            .await
            .unwrap();
    }
    let authoritative_generation = fixture.manager.current_snapshot().generation();

    let turn = second.lease_snapshot_for_turn().await.unwrap();

    assert_eq!(turn.generation(), authoritative_generation);
    assert!(turn.snapshot().packages().is_empty());
    assert!(
        turn.snapshot()
            .inactive()
            .iter()
            .any(|item| { item.package.descriptor.id.as_str() == "com.example.calendar" })
    );
    let max_generation: i64 = sqlx::query_scalar("SELECT MAX(generation) FROM skill_snapshots")
        .fetch_one(fixture.state.pool())
        .await
        .unwrap();
    assert_eq!(
        u64::try_from(max_generation).unwrap(),
        authoritative_generation
    );
    let publications: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_audit_log WHERE operation = 'open_skill_revision_circuit'",
    )
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(publications, 1);
}

#[tokio::test]
async fn startup_preserves_a_revision_repaired_after_failed_verification() {
    let faults = SkillStoreTestFaults::default();
    let gate = faults.gate_once(SkillStoreFaultPoint::RecoveryBeforeQuarantine);
    let fixture = AuthoringFixture::with_faults(faults).await;
    activate_new_revision(&fixture, "1.0.0").await;
    let revision = activate_new_revision(&fixture, "2.0.0").await;
    let record = fixture
        .state
        .get_revision(&revision)
        .await
        .unwrap()
        .unwrap();
    let path = std::path::PathBuf::from(&record.storage_path);
    let descriptor = path.join("general-agent.json");
    let original = tokio::fs::read(&descriptor).await.unwrap();
    make_file_writable(&descriptor).await;
    tokio::fs::write(&descriptor, b"{}").await.unwrap();
    assert!(
        fixture
            .store
            .prepare_invalid_managed_revision(&record)
            .await
            .unwrap()
            .is_some()
    );
    let manager = fixture.manager.clone();
    let mut reconcile = tokio::spawn(async move { manager.startup_reconcile().await });

    tokio::select! {
        _ = gate.wait_entered() => {}
        result = &mut reconcile => panic!("startup ended before quarantine gate: {result:?}"),
        _ = tokio::time::sleep(std::time::Duration::from_secs(2)) => {
            panic!("startup must reach the pre-quarantine gate")
        }
    }
    tokio::fs::write(&descriptor, original).await.unwrap();
    gate.release().await;
    tokio::time::timeout(std::time::Duration::from_secs(2), reconcile)
        .await
        .expect("startup reconcile must finish after quarantine gate release")
        .unwrap()
        .unwrap();

    assert_eq!(
        fixture
            .state
            .get_revision(&revision)
            .await
            .unwrap()
            .unwrap()
            .status,
        crate::skill_state::SkillRevisionStatus::Managed
    );
    assert!(path.is_dir());
    assert!(!fixture.store.paths().quarantine.join(&revision).exists());
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        fixture.manager.startup_reconcile(),
    )
    .await
    .expect("repeated startup reconcile must remain idempotent")
    .unwrap();
    let diagnostics: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_maintenance_diagnostics WHERE operation = 'snapshot_member_changed_before_quarantine' AND revision_id = ?",
    )
    .bind(&revision)
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(diagnostics, 1);
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

#[tokio::test]
async fn cleanup_rechecks_an_approval_created_after_protection_collection() {
    let faults = SkillStoreTestFaults::default();
    let gate = faults.gate_once(SkillStoreFaultPoint::CleanupBeforePrepare);
    let fixture = AuthoringFixture::with_faults(faults).await;
    let retained = activate_new_revision(&fixture, "1.0.0").await;
    activate_new_revision(&fixture, "2.0.0").await;
    make_revision_cleanup_eligible(&fixture, &retained).await;
    let retained_path = std::path::PathBuf::from(
        fixture
            .state
            .get_revision(&retained)
            .await
            .unwrap()
            .unwrap()
            .storage_path,
    );
    let manager = fixture.manager.clone();
    let cleanup = tokio::spawn(async move { manager.cleanup_unreferenced_revisions().await });
    gate.wait_entered().await;

    let second_state = fixture.second_state_connection().await;
    let second_store = crate::skill_store::SkillRevisionStore::new(
        fixture.store.paths().clone(),
        second_state.clone(),
    );
    let second_manager = manager_for_revision_store(second_store.clone()).await;
    let mut policy = SkillManagementPolicy::owner_only();
    policy.rollback_approval_required = true;
    let second_service =
        OwnerSkillManagementService::new(second_manager, second_store, second_state, policy);
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    let outcome = second_service
        .rollback_managed_skill(
            &fixture.actor([crate::skill_policy::SkillGrant::Rollback]),
            &package_id,
            &retained,
        )
        .await
        .unwrap();
    let crate::skill_management::SkillRollbackOutcome::ApprovalRequired(approval) = outcome else {
        panic!("rollback must create a pending approval")
    };
    gate.release().await;

    let report = cleanup.await.unwrap().unwrap();
    assert!(report.retained_revisions.contains(&retained));
    assert!(retained_path.is_dir());
    assert!(
        fixture
            .state
            .get_revision(&retained)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(
        fixture
            .state
            .get_approval(&approval.approval_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        crate::skill_state::SkillApprovalStatus::Pending
    );
}

#[tokio::test]
async fn rollback_approval_rejects_a_matching_pending_cleanup_job() {
    let fixture = AuthoringFixture::with_faults(SkillStoreTestFaults::default()).await;
    let retained = activate_new_revision(&fixture, "1.0.0").await;
    activate_new_revision(&fixture, "2.0.0").await;
    make_revision_cleanup_eligible(&fixture, &retained).await;
    sqlx::query("DELETE FROM skill_snapshots WHERE generation = 2 AND status = 'candidate'")
        .execute(fixture.state.pool())
        .await
        .unwrap();
    let record = fixture
        .state
        .get_revision(&retained)
        .await
        .unwrap()
        .unwrap();
    assert!(
        fixture
            .state
            .prepare_revision_cleanup(&record)
            .await
            .unwrap()
    );
    let mut policy = SkillManagementPolicy::owner_only();
    policy.rollback_approval_required = true;
    let service = OwnerSkillManagementService::new(
        fixture.manager.clone(),
        fixture.store.clone(),
        fixture.state.clone(),
        policy,
    );
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();

    let error = service
        .rollback_managed_skill(
            &fixture.actor([crate::skill_policy::SkillGrant::Rollback]),
            &package_id,
            &retained,
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
}

async fn make_revision_cleanup_eligible(fixture: &AuthoringFixture, revision_id: &str) {
    sqlx::query("UPDATE skill_revision_retention SET retain_until = ? WHERE revision_id = ?")
        .bind((Utc::now() - Duration::seconds(1)).to_rfc3339())
        .bind(revision_id)
        .execute(fixture.state.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE skill_snapshots SET status = 'candidate' WHERE generation = 2")
        .execute(fixture.state.pool())
        .await
        .unwrap();
}

async fn manager_for_revision_store(store: crate::skill_store::SkillRevisionStore) -> SkillManager {
    SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(ManagedSkillSource::from_store(store)) as Arc<dyn SkillSource>],
        platform: PlatformId::Server,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
    })
    .await
    .unwrap()
}
