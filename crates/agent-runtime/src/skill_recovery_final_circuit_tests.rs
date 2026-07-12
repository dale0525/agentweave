use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_management::{CreateSkillDraftRequest, OwnerSkillManagementService};
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use crate::skill_recovery_tests::activate_new_revision;
use crate::skill_source::{ManagedSkillSource, SkillSource};
use crate::skill_store::{SkillStoreFaultPoint, SkillStoreTestFaults};
use crate::tools::ToolSource;
use chrono::{Duration, Utc};
use std::sync::Arc;

#[tokio::test]
async fn open_success_interleaving_reopens_without_stranding_the_omission() {
    let faults = SkillStoreTestFaults::default();
    let gate = faults.gate_once(SkillStoreFaultPoint::CircuitAfterStateTransition);
    let fixture = AuthoringFixture::with_faults(faults).await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let old_lease = fixture.manager.lease_snapshot();
    let source = managed_source("com.example.calendar", &revision);
    let initial_generation = old_lease.generation();
    let mut events = fixture.service.subscribe_events();

    record_failures(&fixture, &source, 2).await;
    let opening_manager = fixture.manager.clone();
    let opening_source = source.clone();
    let opening = tokio::spawn(async move {
        opening_manager
            .record_execution_result(&opening_source, false)
            .await
    });
    gate.wait_entered().await;

    let success_manager = fixture.manager.clone();
    let success_source = source.clone();
    let success = tokio::spawn(async move {
        success_manager
            .record_execution_result(&success_source, true)
            .await
    });
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    gate.release().await;

    opening.await.unwrap().unwrap();
    success.await.unwrap().unwrap();
    assert_eq!(old_lease.snapshot().packages().len(), 1);
    assert_eq!(fixture.manager.current_snapshot().packages().len(), 1);

    record_failures(&fixture, &source, 3).await;

    assert!(
        fixture
            .manager
            .lease_snapshot()
            .snapshot()
            .packages()
            .is_empty()
    );
    let omission: (i64, Option<i64>) = sqlx::query_as(
        "SELECT omitted_generation, consumed_generation FROM skill_circuit_omissions WHERE revision_id = ?",
    )
    .bind(&revision)
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(omission.0, i64::try_from(initial_generation + 3).unwrap());
    assert!(omission.1.is_none());

    let publications: Vec<(String, i64)> = sqlx::query_as(
        "SELECT operation, json_extract(metadata_json, '$.generation') FROM skill_audit_log WHERE operation IN ('open_skill_revision_circuit', 'close_skill_revision_circuit') ORDER BY created_at, id",
    )
    .fetch_all(fixture.state.pool())
    .await
    .unwrap();
    assert_eq!(
        publications,
        vec![
            (
                "open_skill_revision_circuit".into(),
                i64::try_from(initial_generation + 1).unwrap()
            ),
            (
                "close_skill_revision_circuit".into(),
                i64::try_from(initial_generation + 2).unwrap()
            ),
            (
                "open_skill_revision_circuit".into(),
                i64::try_from(initial_generation + 3).unwrap()
            ),
        ]
    );
    for generation in (initial_generation + 1)..=(initial_generation + 3) {
        assert_eq!(
            events.recv().await.unwrap(),
            crate::events::RuntimeEvent::SkillSnapshotPublished { generation }
        );
    }
    assert!(matches!(
        events.try_recv(),
        Err(tokio::sync::broadcast::error::TryRecvError::Empty)
    ));
}

#[tokio::test]
async fn stale_manager_open_success_interleaving_reconciles_as_an_exact_noop() {
    let faults = SkillStoreTestFaults::default();
    let gate = faults.gate_once(SkillStoreFaultPoint::CircuitAfterStateTransition);
    let fixture = AuthoringFixture::with_faults(faults).await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let stale = manager_for_store(&fixture).await;
    let _stale_service = bind_manager(&fixture, stale.clone());
    stale.startup_reconcile().await.unwrap();
    let source = managed_source("com.example.calendar", &revision);
    let initial_generation = fixture.manager.current_snapshot().generation();

    record_failures(&fixture, &source, 2).await;
    let opening_manager = fixture.manager.clone();
    let opening_source = source.clone();
    let opening = tokio::spawn(async move {
        opening_manager
            .record_execution_result(&opening_source, false)
            .await
    });
    gate.wait_entered().await;

    stale.record_execution_result(&source, true).await.unwrap();
    gate.release().await;
    opening.await.unwrap().unwrap();

    assert_eq!(
        fixture.manager.current_snapshot().generation(),
        initial_generation
    );
    assert!(
        fixture
            .state
            .circuit_omission(&revision)
            .await
            .unwrap()
            .is_none()
    );

    record_failures(&fixture, &source, 3).await;
    assert_eq!(
        fixture.manager.current_snapshot().generation(),
        initial_generation + 1
    );
    assert!(fixture.manager.current_snapshot().packages().is_empty());
    assert!(
        !fixture
            .state
            .circuit_omission(&revision)
            .await
            .unwrap()
            .unwrap()
            .consumed
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
async fn two_revision_in_process_expiry_consumes_every_omission_before_reopen() {
    let fixture = AuthoringFixture::new().await;
    let revisions = activate_two_packages(&fixture).await;
    let sources = [
        managed_source("com.example.calendar", &revisions[0]),
        managed_source("com.example.tasks", &revisions[1]),
    ];
    open_each(&fixture, &sources).await;
    let open_generation = fixture.manager.current_snapshot().generation();
    expire_all(&fixture, &revisions).await;
    let mut events = fixture.service.subscribe_events();

    let restored = fixture.manager.lease_snapshot_for_turn().await.unwrap();

    assert_eq!(restored.generation(), open_generation + 1);
    assert_eq!(restored.snapshot().packages().len(), 2);
    assert_all_omissions(&fixture, &revisions, true).await;
    assert_eq!(
        events.recv().await.unwrap(),
        crate::events::RuntimeEvent::SkillSnapshotPublished {
            generation: open_generation + 1,
        }
    );
    assert!(events.try_recv().is_err());

    open_each(&fixture, &sources).await;
    assert_eq!(
        fixture.manager.current_snapshot().generation(),
        open_generation + 3
    );
    assert!(fixture.manager.current_snapshot().packages().is_empty());
    assert_all_omissions(&fixture, &revisions, false).await;
}

#[tokio::test]
async fn two_revision_restart_expiry_consumes_every_omission_before_reopen() {
    let fixture = AuthoringFixture::new().await;
    let revisions = activate_two_packages(&fixture).await;
    let sources = [
        managed_source("com.example.calendar", &revisions[0]),
        managed_source("com.example.tasks", &revisions[1]),
    ];
    open_each(&fixture, &sources).await;
    let open_generation = fixture.manager.current_snapshot().generation();
    expire_all(&fixture, &revisions).await;
    let restarted = manager_for_store(&fixture).await;
    let service = bind_manager(&fixture, restarted.clone());
    let mut events = service.subscribe_events();

    restarted.startup_reconcile().await.unwrap();

    assert_eq!(
        restarted.current_snapshot().generation(),
        open_generation + 1
    );
    assert_eq!(restarted.current_snapshot().packages().len(), 2);
    assert_all_omissions(&fixture, &revisions, true).await;
    assert_eq!(
        events.recv().await.unwrap(),
        crate::events::RuntimeEvent::SkillRecoveryCompleted {
            status: crate::skill_recovery::RecoveryStatus::NewSnapshotPublished,
            generation: open_generation + 1,
        }
    );
    assert!(events.try_recv().is_err());

    for source in &sources {
        for _ in 0..3 {
            restarted
                .record_execution_result(source, false)
                .await
                .unwrap();
        }
    }
    assert_eq!(
        restarted.current_snapshot().generation(),
        open_generation + 3
    );
    assert!(restarted.current_snapshot().packages().is_empty());
    assert_all_omissions(&fixture, &revisions, false).await;
}

async fn record_failures(fixture: &AuthoringFixture, source: &ToolSource, count: usize) {
    for _ in 0..count {
        fixture
            .manager
            .record_execution_result(source, false)
            .await
            .unwrap();
    }
}

async fn activate_two_packages(fixture: &AuthoringFixture) -> [String; 2] {
    let calendar = activate_new_revision(fixture, "1.0.0").await;
    let tasks = fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse("com.example.tasks").unwrap(),
                display_name: "Tasks".into(),
                description: "Guide task planning.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &tasks.revision_id)
        .await
        .unwrap();
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &tasks.revision_id)
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
    [calendar, tasks.revision_id]
}

async fn open_each(fixture: &AuthoringFixture, sources: &[ToolSource; 2]) {
    for source in sources {
        record_failures(fixture, source, 3).await;
    }
}

async fn expire_all(fixture: &AuthoringFixture, revisions: &[String; 2]) {
    for revision in revisions {
        sqlx::query("UPDATE skill_circuit_state SET open_until = ? WHERE revision_id = ?")
            .bind((Utc::now() - Duration::seconds(1)).to_rfc3339())
            .bind(revision)
            .execute(fixture.state.pool())
            .await
            .unwrap();
    }
}

async fn assert_all_omissions(fixture: &AuthoringFixture, revisions: &[String; 2], consumed: bool) {
    for revision in revisions {
        assert_eq!(
            fixture
                .state
                .circuit_omission(revision)
                .await
                .unwrap()
                .unwrap()
                .consumed,
            consumed
        );
    }
}

async fn manager_for_store(fixture: &AuthoringFixture) -> SkillManager {
    SkillManager::new_deferred_managed(SkillManagerConfig {
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

fn bind_manager(fixture: &AuthoringFixture, manager: SkillManager) -> OwnerSkillManagementService {
    OwnerSkillManagementService::new(
        manager,
        fixture.store.clone(),
        fixture.state.clone(),
        SkillManagementPolicy::owner_only(),
    )
}

fn managed_source(package_id: &str, revision_id: &str) -> ToolSource {
    ToolSource::RuntimeSkill {
        skill_name: "managed-runtime".into(),
        package_id: package_id.into(),
        revision_id: Some(revision_id.into()),
    }
}
