use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_policy::SkillGrant;
use crate::skill_recovery_tests::activate_new_revision;
use crate::skill_state::{
    SkillInstallStatus, SkillInstallationRecord, SkillStateBoundaryError, SkillStateStore,
};
use crate::skill_state_lifecycle::{ExactLifecyclePublication, LifecycleTarget};
use crate::skill_store::{SkillStoreFaultPoint, SkillStoreTestFaults};
use crate::storage::Storage;
use chrono::Utc;

#[tokio::test]
async fn cancelled_disable_waiter_finishes_post_commit_publication_in_process() {
    let faults = SkillStoreTestFaults::default();
    let gate = faults.gate_once(SkillStoreFaultPoint::LifecycleAfterDurableCommit);
    let fixture = AuthoringFixture::with_faults(faults).await;
    activate_new_revision(&fixture, "1.0.0").await;
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    let service = fixture.service.clone();
    let actor = fixture.actor([SkillGrant::Disable]);
    let package = package_id.clone();
    let waiter = tokio::spawn(async move { service.disable_managed_skill(&actor, &package).await });

    gate.wait_entered().await;
    waiter.abort();
    gate.release().await;

    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let installation = fixture
                .state
                .get_installation(&package_id)
                .await
                .unwrap()
                .unwrap();
            if installation.status == SkillInstallStatus::Disabled
                && fixture.manager.current_snapshot().generation() == 3
                && fixture.manager.current_snapshot().packages().is_empty()
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached lifecycle publication must converge without restart");
}

#[tokio::test]
async fn concurrent_lifecycle_publications_cas_the_authoritative_snapshot() {
    let database = tempfile::NamedTempFile::new().unwrap();
    let url = format!("sqlite://{}?mode=rwc", database.path().display());
    let first = SkillStateStore::new(Storage::connect(&url).await.unwrap());
    let second = SkillStateStore::new(Storage::connect(&url).await.unwrap());
    let package_a = crate::skill_package::SkillPackageId::parse("com.example.alpha").unwrap();
    let package_b = crate::skill_package::SkillPackageId::parse("com.example.beta").unwrap();
    let revision_a = "11111111-1111-4111-8111-111111111111";
    let revision_b = "22222222-2222-4222-8222-222222222222";
    seed_lifecycle_state(&first, &package_a, revision_a).await;
    seed_lifecycle_state(&first, &package_b, revision_b).await;
    let previous_members = serde_json::json!([
        {"packageId": package_a.as_str(), "version": "1.0.0", "contentHash": "hash-alpha", "layer": "managed", "revisionId": revision_a},
        {"packageId": package_b.as_str(), "version": "1.0.0", "contentHash": "hash-beta", "layer": "managed", "revisionId": revision_b}
    ]);
    sqlx::query("INSERT INTO skill_snapshots (generation, status, members_json, created_at, activated_at) VALUES (7, 'active', ?, ?, ?)")
        .bind(serde_json::to_string(&previous_members).unwrap())
        .bind(Utc::now().to_rfc3339())
        .bind(Utc::now().to_rfc3339())
        .execute(first.pool())
        .await
        .unwrap();
    let installation_a = first.get_installation(&package_a).await.unwrap().unwrap();
    let installation_b = second.get_installation(&package_b).await.unwrap().unwrap();
    let members_without_a = serde_json::json!([previous_members[1].clone()]);
    let members_without_b = serde_json::json!([previous_members[0].clone()]);

    let publish_a = commit_disable(
        first.clone(),
        package_a,
        installation_a,
        previous_members.clone(),
        members_without_a,
    );
    let publish_b = commit_disable(
        second.clone(),
        package_b,
        installation_b,
        previous_members,
        members_without_b,
    );
    let (result_a, result_b) = tokio::join!(publish_a, publish_b);

    assert_ne!(result_a.is_ok(), result_b.is_ok());
    let error = result_a.err().or_else(|| result_b.err()).unwrap();
    assert!(matches!(
        error.downcast_ref::<SkillStateBoundaryError>(),
        Some(SkillStateBoundaryError::Conflict(_))
    ));
    let active_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM skill_snapshots WHERE status = 'active'")
            .fetch_one(first.pool())
            .await
            .unwrap();
    assert_eq!(active_count, 1);
}

#[tokio::test]
async fn rollback_policy_creates_exact_bound_different_actor_single_use_approval() {
    let fixture = AuthoringFixture::new().await;
    let first = activate_new_revision(&fixture, "1.0.0").await;
    let second = activate_new_revision(&fixture, "2.0.0").await;
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    let mut policy = crate::skill_policy::SkillManagementPolicy::owner_only();
    policy.rollback_approval_required = true;
    let service = crate::skill_management::OwnerSkillManagementService::new(
        fixture.manager.clone(),
        fixture.store.clone(),
        fixture.state.clone(),
        policy,
    );

    let outcome = service
        .rollback_managed_skill(&fixture.actor([SkillGrant::Rollback]), &package_id, &first)
        .await
        .unwrap();
    let crate::skill_management::SkillRollbackOutcome::ApprovalRequired(approval) = outcome else {
        panic!("rollback policy must return a pending approval")
    };
    let binding = fixture
        .state
        .approval_binding_value(&approval.approval_id)
        .await
        .unwrap();
    assert_eq!(binding["packageId"], package_id.as_str());
    assert_eq!(binding["targetRevisionId"], first);
    assert_eq!(binding["currentRevisionId"], second);
    assert_eq!(binding["snapshotGeneration"], 3);
    assert!(
        binding["contentHash"]
            .as_str()
            .is_some_and(|hash| !hash.is_empty())
    );
    assert_eq!(fixture.manager.current_snapshot().generation(), 3);

    let same_actor = service
        .approve_pending_skill_operation(
            &approval.approval_id,
            &fixture.actor([SkillGrant::Rollback]),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        same_actor.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
    let report = service
        .approve_pending_skill_operation(
            &approval.approval_id,
            &crate::skill_policy::ActorContext::owner("approver-2", [SkillGrant::Rollback]),
        )
        .await
        .unwrap();
    assert_eq!(report.active_generation, 4);
    assert_eq!(
        fixture
            .state
            .get_installation(&package_id)
            .await
            .unwrap()
            .unwrap()
            .active_revision_id
            .as_deref(),
        Some(first.as_str())
    );
    assert!(
        service
            .approve_pending_skill_operation(
                &approval.approval_id,
                &crate::skill_policy::ActorContext::owner("approver-3", [SkillGrant::Rollback]),
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rollback_approval_conflicts_after_snapshot_generation_changes() {
    let fixture = AuthoringFixture::new().await;
    let first = activate_new_revision(&fixture, "1.0.0").await;
    activate_new_revision(&fixture, "2.0.0").await;
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    let mut policy = crate::skill_policy::SkillManagementPolicy::owner_only();
    policy.rollback_approval_required = true;
    let guarded = crate::skill_management::OwnerSkillManagementService::new(
        fixture.manager.clone(),
        fixture.store.clone(),
        fixture.state.clone(),
        policy,
    );
    let outcome = guarded
        .rollback_managed_skill(&fixture.actor([SkillGrant::Rollback]), &package_id, &first)
        .await
        .unwrap();
    let crate::skill_management::SkillRollbackOutcome::ApprovalRequired(approval) = outcome else {
        panic!("rollback policy must return approval")
    };
    fixture
        .service
        .rollback_managed_skill(&fixture.actor([SkillGrant::Rollback]), &package_id, &first)
        .await
        .unwrap();

    let error = guarded
        .approve_pending_skill_operation(
            &approval.approval_id,
            &crate::skill_policy::ActorContext::owner("approver-2", [SkillGrant::Rollback]),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
}

#[tokio::test]
async fn protected_removal_resolution_denies_before_private_binding_lookup() {
    let fixture = AuthoringFixture::new().await;
    activate_new_revision(&fixture, "1.0.0").await;
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    let approval = fixture
        .service
        .request_removal(&fixture.actor([SkillGrant::DeleteManaged]), &package_id)
        .await
        .unwrap();
    sqlx::query("DELETE FROM skill_approval_bindings WHERE approval_id = ?")
        .bind(&approval.approval_id)
        .execute(fixture.state.pool())
        .await
        .unwrap();
    let protected = crate::skill_management::OwnerSkillManagementService::new(
        fixture.manager.clone(),
        fixture.store.clone(),
        fixture.state.clone(),
        crate::skill_policy::SkillManagementPolicy::owner_only().protect(package_id),
    );

    let error = protected
        .approve_pending_skill_operation(
            &approval.approval_id,
            &crate::skill_policy::ActorContext::owner("approver-2", [SkillGrant::DeleteManaged]),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Denied { .. })
    ));
}

async fn seed_lifecycle_state(
    state: &SkillStateStore,
    package_id: &crate::skill_package::SkillPackageId,
    revision_id: &str,
) {
    let suffix = package_id.as_str().rsplit('.').next().unwrap();
    let now = Utc::now().to_rfc3339();
    sqlx::query("INSERT INTO skill_revisions (revision_id, package_id, version, content_hash, storage_path, descriptor_json, validation_json, created_by, created_at, lifecycle_status) VALUES (?, ?, '1.0.0', ?, ?, '{}', '{}', 'owner', ?, 'managed')")
        .bind(revision_id)
        .bind(package_id.as_str())
        .bind(format!("hash-{suffix}"))
        .bind(format!("managed/{suffix}"))
        .bind(&now)
        .execute(state.pool())
        .await
        .unwrap();
    sqlx::query("INSERT INTO skill_installations (package_id, source_layer, active_revision_id, enabled, trust_level, install_status, installed_at, updated_at) VALUES (?, 'managed', ?, 1, 'approved', 'active', ?, ?)")
        .bind(package_id.as_str())
        .bind(revision_id)
        .bind(&now)
        .bind(&now)
        .execute(state.pool())
        .await
        .unwrap();
}

async fn commit_disable(
    state: SkillStateStore,
    package_id: crate::skill_package::SkillPackageId,
    installation: SkillInstallationRecord,
    previous_members: serde_json::Value,
    members: serde_json::Value,
) -> anyhow::Result<()> {
    state
        .commit_exact_lifecycle_publication(ExactLifecyclePublication {
            actor_id: "owner",
            operation: "disable_managed_skill",
            package_id: &package_id,
            expected_installation: &installation,
            target: LifecycleTarget::Disabled,
            approval: None,
            previous_generation: 7,
            previous_members,
            generation: 8,
            members,
        })
        .await
}
