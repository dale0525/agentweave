use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_management::{
    OwnerSkillManagementService, SkillDraftValidation, SkillManagementError, SkillRollbackOutcome,
};
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use crate::skill_recovery::{parse_snapshot_members, snapshot_members};
use crate::skill_source::ManagedSkillSource;
use crate::skill_state::{
    SkillInstallStatus, SkillLayerRecord, SkillRevisionRecord, SkillSnapshotStatus, SkillStateStore,
};
use crate::skill_store::{
    SkillRevisionStore, SkillStoreFaultPoint, SkillStoreLimits, SkillStoreTestFaults,
};
use serde_json::json;
use std::sync::Arc;
use tempfile::{TempDir, tempdir};

const PACKAGE_ID: &str = "com.example.rollback-cleanup";

struct RollbackCleanupFixture {
    authoring: AuthoringFixture,
    faults: SkillStoreTestFaults,
    package_id: SkillPackageId,
    target_revision: String,
    current_revision: String,
    target_record: SkillRevisionRecord,
}

impl RollbackCleanupFixture {
    async fn new() -> Self {
        let faults = SkillStoreTestFaults::default();
        let mut authoring = AuthoringFixture::with_faults(faults.clone()).await;
        let mut policy = SkillManagementPolicy::owner_only();
        policy.allowed_kinds.insert(SkillPackageKind::NativeRuntime);
        authoring.service = OwnerSkillManagementService::new(
            authoring.manager.clone(),
            authoring.store.clone(),
            authoring.state.clone(),
            policy,
        );
        let package_id = SkillPackageId::parse(PACKAGE_ID).unwrap();
        let target_revision = create_revision(&authoring, "rollback-target", "1.0.0").await;
        authoring
            .state
            .activate_revision(
                &package_id,
                &target_revision,
                SkillLayerRecord::Managed,
                "fixture",
            )
            .await
            .unwrap();
        authoring.manager.startup_reconcile().await.unwrap();
        let current_revision = create_revision(&authoring, "current", "2.0.0").await;
        publish_revision(
            &authoring.state,
            &authoring.manager,
            &package_id,
            &current_revision,
        )
        .await;
        make_cleanup_eligible(&authoring.state, &target_revision).await;
        let target_record = authoring
            .state
            .get_revision(&target_revision)
            .await
            .unwrap()
            .unwrap();
        Self {
            authoring,
            faults,
            package_id,
            target_revision,
            current_revision,
            target_record,
        }
    }

    async fn independent_manager(&self, faults: SkillStoreTestFaults) -> SkillManager {
        let state = self.authoring.second_state_connection().await;
        let store = SkillRevisionStore::with_test_faults(
            self.authoring.store.paths().clone(),
            state.clone(),
            SkillStoreLimits::default(),
            faults,
        );
        let manager = SkillManager::new(SkillManagerConfig {
            sources: vec![Arc::new(ManagedSkillSource::from_store(store.clone()))],
            platform: PlatformId::Server,
            capabilities: CapabilitySet::from_names(Vec::<String>::new()),
            protected_packages: Vec::new(),
            allowed_overrides: Vec::new(),
            runtime_version: "0.1.0".parse().unwrap(),
        })
        .await
        .unwrap();
        let mut policy = SkillManagementPolicy::owner_only();
        policy.allowed_kinds.insert(SkillPackageKind::NativeRuntime);
        let _service = OwnerSkillManagementService::new(manager.clone(), store, state, policy);
        manager
            .converge_to_authoritative_generation()
            .await
            .unwrap();
        manager
    }

    fn rollback_actor(&self) -> ActorContext {
        ActorContext::owner("rollback-owner", [SkillGrant::Rollback])
    }
}

#[tokio::test]
async fn prepared_cleanup_makes_default_rollback_fail_closed_without_publication() {
    let fixture = RollbackCleanupFixture::new().await;
    let cleanup_faults = SkillStoreTestFaults::default();
    cleanup_faults.fail_once(SkillStoreFaultPoint::CleanupBeforeTreeDelete);
    let before_delete = cleanup_faults.gate_once(SkillStoreFaultPoint::CleanupBeforeTreeDelete);
    let cleanup_manager = fixture.independent_manager(cleanup_faults).await;
    let retry_cleanup_manager = cleanup_manager.clone();
    let rollback_attempt = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::RevisionLockAttempt);
    let cleanup =
        tokio::spawn(async move { cleanup_manager.cleanup_unreferenced_revisions().await });
    wait_for_gate(
        &before_delete,
        "cleanup must prepare the target before delete",
    )
    .await;
    assert_eq!(pending_cleanup_count(&fixture).await, 1);

    let service = fixture.authoring.service.clone();
    let actor = fixture.rollback_actor();
    let package_id = fixture.package_id.clone();
    let target_revision = fixture.target_revision.clone();
    let mut rollback = tokio::spawn(async move {
        service
            .rollback_managed_skill(&actor, &package_id, &target_revision)
            .await
    });
    wait_for_gate(
        &rollback_attempt,
        "rollback must join the exact revision lock protocol",
    )
    .await;
    rollback_attempt.release().await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut rollback)
            .await
            .is_err(),
        "rollback completed while cleanup held the target revision lock"
    );
    assert_active_revision(&fixture, &fixture.current_revision).await;
    assert!(std::path::Path::new(&fixture.target_record.storage_path).is_dir());

    before_delete.release().await;
    cleanup.await.unwrap().unwrap_err();
    assert_eq!(pending_cleanup_count(&fixture).await, 1);
    assert_eq!(
        fixture
            .authoring
            .state
            .get_revision(&fixture.target_revision)
            .await
            .unwrap()
            .as_ref(),
        Some(&fixture.target_record)
    );
    assert!(std::path::Path::new(&fixture.target_record.storage_path).is_dir());
    let error = rollback.await.unwrap().unwrap_err();
    assert!(matches!(
        error.downcast_ref::<SkillManagementError>(),
        Some(SkillManagementError::Conflict { resource }) if *resource == "rollback publication"
    ));
    assert_active_revision(&fixture, &fixture.current_revision).await;

    let cleanup_report = retry_cleanup_manager
        .cleanup_unreferenced_revisions()
        .await
        .unwrap();
    assert!(
        cleanup_report
            .deleted_revisions
            .contains(&fixture.target_revision)
    );
    assert!(
        fixture
            .authoring
            .state
            .get_revision(&fixture.target_revision)
            .await
            .unwrap()
            .is_none()
    );
    assert!(!std::path::Path::new(&fixture.target_record.storage_path).exists());
    assert_eq!(pending_cleanup_count(&fixture).await, 0);
}

#[tokio::test]
async fn rollback_first_holds_target_lock_through_publish_then_cleanup_retains_it() {
    let fixture = RollbackCleanupFixture::new().await;
    let after_commit = fixture
        .faults
        .gate_once(SkillStoreFaultPoint::LifecycleAfterDurableCommit);
    let cleanup_faults = SkillStoreTestFaults::default();
    let cleanup_attempt = cleanup_faults.gate_once(SkillStoreFaultPoint::RevisionLockAttempt);
    let cleanup_manager = fixture.independent_manager(cleanup_faults).await;
    let mut cleanup =
        tokio::spawn(async move { cleanup_manager.cleanup_unreferenced_revisions().await });
    wait_for_gate(
        &cleanup_attempt,
        "cleanup must finish its outer scan before target lock acquisition",
    )
    .await;

    let service = fixture.authoring.service.clone();
    let actor = fixture.rollback_actor();
    let package_id = fixture.package_id.clone();
    let target_revision = fixture.target_revision.clone();
    let rollback = tokio::spawn(async move {
        service
            .rollback_managed_skill(&actor, &package_id, &target_revision)
            .await
    });
    wait_for_gate(
        &after_commit,
        "rollback must reach its durable commit while holding the target lock",
    )
    .await;
    cleanup_attempt.release().await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(100), &mut cleanup)
            .await
            .is_err(),
        "cleanup completed before rollback published and released the target revision lock"
    );

    after_commit.release().await;
    let outcome = rollback.await.unwrap().unwrap();
    let SkillRollbackOutcome::Published(report) = outcome else {
        panic!("default rollback must publish after the lock gate")
    };
    assert_eq!(report.active_revision_id, fixture.target_revision);
    let cleanup_report = cleanup.await.unwrap().unwrap();
    assert!(
        cleanup_report
            .retained_revisions
            .contains(&fixture.target_revision)
    );
    assert!(
        !cleanup_report
            .deleted_revisions
            .contains(&fixture.target_revision)
    );
    assert_active_revision(&fixture, &fixture.target_revision).await;
    assert_eq!(
        fixture
            .authoring
            .state
            .get_revision(&fixture.target_revision)
            .await
            .unwrap()
            .as_ref(),
        Some(&fixture.target_record)
    );
    assert!(std::path::Path::new(&fixture.target_record.storage_path).is_dir());
    assert_eq!(pending_cleanup_count(&fixture).await, 0);

    let next_turn = fixture
        .authoring
        .manager
        .lease_snapshot_for_turn()
        .await
        .unwrap();
    let result = next_turn
        .snapshot()
        .registry()
        .execute("managed_tool", json!({}))
        .await
        .unwrap();
    assert_eq!(result["revision"], "rollback-target");
}

async fn create_revision(fixture: &AuthoringFixture, label: &str, version: &str) -> String {
    let package = write_runtime_package(label, version).await;
    let staged = fixture
        .store
        .create_staging_revision(package.path(), "fixture")
        .await
        .unwrap();
    let validation = SkillDraftValidation {
        ok: true,
        errors: Vec::new(),
        warnings: Vec::new(),
        required_tools: Vec::new(),
        required_connectors: Vec::new(),
        dependencies: Vec::new(),
        required_capabilities: Vec::new(),
        resolver_status: "active".into(),
        resolver_errors: Vec::new(),
        permission_diff: json!({}),
        revision_id: staged.revision_id.clone(),
        content_hash: staged.content_hash.clone(),
        snapshot_generation: fixture.manager.current_snapshot().generation(),
    };
    fixture
        .state
        .update_revision_validation(
            &staged.revision_id,
            serde_json::to_value(validation).unwrap(),
        )
        .await
        .unwrap();
    fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap()
        .revision_id
}

async fn publish_revision(
    state: &SkillStateStore,
    manager: &SkillManager,
    package_id: &SkillPackageId,
    revision_id: &str,
) {
    let active = state
        .snapshot_with_status(SkillSnapshotStatus::Active)
        .await
        .unwrap()
        .unwrap();
    state
        .activate_revision(
            package_id,
            revision_id,
            SkillLayerRecord::Managed,
            "fixture",
        )
        .await
        .unwrap();
    manager.reload().await.unwrap();
    let candidate = manager.current_snapshot();
    state
        .persist_recovery_candidate(
            &active,
            candidate.generation(),
            &snapshot_members(&candidate),
        )
        .await
        .unwrap();
}

async fn make_cleanup_eligible(state: &SkillStateStore, revision_id: &str) {
    sqlx::query("UPDATE skill_revision_retention SET retain_until = ? WHERE revision_id = ?")
        .bind("2000-01-01T00:00:00Z")
        .bind(revision_id)
        .execute(state.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE skill_snapshots SET status = 'candidate' WHERE status = 'last_known_good'")
        .execute(state.pool())
        .await
        .unwrap();
}

async fn assert_active_revision(fixture: &RollbackCleanupFixture, expected_revision: &str) {
    let installation = fixture
        .authoring
        .state
        .get_installation(&fixture.package_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(installation.status, SkillInstallStatus::Active);
    assert!(installation.enabled);
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(expected_revision)
    );
    let active = fixture
        .authoring
        .state
        .snapshot_with_status(SkillSnapshotStatus::Active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(only_revision(active.members_json), expected_revision);
    assert_eq!(
        only_revision(snapshot_members(
            &fixture.authoring.manager.current_snapshot()
        )),
        expected_revision
    );
}

fn only_revision(members: serde_json::Value) -> String {
    let members = parse_snapshot_members(members).unwrap();
    assert_eq!(members.len(), 1);
    members[0].revision_id.clone().unwrap()
}

async fn pending_cleanup_count(fixture: &RollbackCleanupFixture) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM skill_revision_cleanup WHERE revision_id = ? AND status = 'pending'",
    )
    .bind(&fixture.target_revision)
    .fetch_one(fixture.authoring.state.pool())
    .await
    .unwrap()
}

async fn wait_for_gate(gate: &crate::skill_store_faults::StoreTestGate, message: &str) {
    tokio::time::timeout(std::time::Duration::from_secs(2), gate.wait_entered())
        .await
        .expect(message);
}

async fn write_runtime_package(label: &str, version: &str) -> TempDir {
    let root = tempdir().unwrap();
    tokio::fs::write(
        root.path().join("agentweave.json"),
        json!({
            "schemaVersion": 1,
            "id": PACKAGE_ID,
            "version": version,
            "displayName": "Rollback cleanup runtime",
            "kind": "native_runtime",
            "package": {"includeInstructions": false, "includeRuntime": true}
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("skill.json"),
        json!({
            "name": "rollback-cleanup-runtime",
            "description": "Rollback cleanup concurrency regression runtime.",
            "version": version,
            "entry": {"type": "command", "command": "sh", "args": ["run.sh"]},
            "tools": [{
                "name": "managed_tool",
                "description": "Return the executing revision bytes.",
                "input_schema": {"type": "object"}
            }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        root.path().join("run.sh"),
        format!("printf '{{\"revision\":\"{label}\"}}'\n"),
    )
    .await
    .unwrap();
    root
}
