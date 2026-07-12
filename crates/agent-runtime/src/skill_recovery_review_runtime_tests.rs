use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_management::OwnerSkillManagementService;
use crate::skill_manager::{SkillManager, SkillManagerConfig};
use crate::skill_policy::SkillManagementPolicy;
use crate::skill_recovery_tests::activate_new_revision;
use crate::skill_source::ManagedSkillSource;
use crate::tools::ToolSource;
use chrono::{Duration, Utc};
use std::sync::Arc;

#[tokio::test]
async fn third_failure_automatically_hides_revision_from_the_next_turn() {
    let fixture = AuthoringFixture::new().await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let generation = fixture.manager.current_snapshot().generation();
    let source = managed_source(&revision);

    for _ in 0..3 {
        fixture
            .manager
            .record_execution_result(&source, false)
            .await
            .unwrap();
    }

    let next_turn = fixture.manager.lease_snapshot();
    assert_eq!(next_turn.generation(), generation + 1);
    assert!(next_turn.snapshot().packages().is_empty());
    assert!(next_turn.snapshot().inactive().iter().any(|item| {
        item.package.descriptor.id.as_str() == "com.example.calendar"
            && item.reason.contains("circuit open")
    }));
}

#[tokio::test]
async fn restart_reconstruction_filters_a_durable_open_circuit() {
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

    let restarted = SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(ManagedSkillSource::from_store(
            fixture.store.clone(),
        ))],
        platform: PlatformId::Server,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
    })
    .await
    .unwrap();
    let _service = OwnerSkillManagementService::new(
        restarted.clone(),
        fixture.store.clone(),
        fixture.state.clone(),
        SkillManagementPolicy::owner_only(),
    );

    restarted.startup_reconcile().await.unwrap();

    assert!(restarted.current_snapshot().packages().is_empty());
    assert!(restarted.current_snapshot().inactive().iter().any(|item| {
        item.package.descriptor.id.as_str() == "com.example.calendar"
            && item.reason.contains("circuit open")
    }));
}

#[tokio::test]
async fn first_turn_after_circuit_expiry_republishes_the_revision() {
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

    let next_turn = fixture.manager.lease_snapshot_for_turn().await.unwrap();

    assert_eq!(next_turn.generation(), open_generation + 1);
    assert_eq!(next_turn.snapshot().packages().len(), 1);
    assert!(next_turn.snapshot().inactive().is_empty());
}

#[tokio::test]
async fn turn_start_enforces_a_durable_open_row_against_stale_active_memory() {
    let fixture = AuthoringFixture::new().await;
    let revision = activate_new_revision(&fixture, "1.0.0").await;
    let stale_generation = fixture.manager.current_snapshot().generation();
    let now = Utc::now();
    sqlx::query(
        "INSERT INTO skill_circuit_state (revision_id, consecutive_failures, open_until, updated_at) VALUES (?, 3, ?, ?)",
    )
    .bind(&revision)
    .bind((now + Duration::minutes(5)).to_rfc3339())
    .bind(now.to_rfc3339())
    .execute(fixture.state.pool())
    .await
    .unwrap();

    let next_turn = fixture.manager.lease_snapshot_for_turn().await.unwrap();

    assert_eq!(next_turn.generation(), stale_generation + 1);
    assert!(next_turn.snapshot().packages().is_empty());
    assert!(next_turn.snapshot().inactive().iter().any(|item| {
        item.package.descriptor.id.as_str() == "com.example.calendar"
            && item.reason.contains("circuit open")
    }));
}

fn managed_source(revision_id: &str) -> ToolSource {
    ToolSource::RuntimeSkill {
        skill_name: "calendar-runtime".into(),
        package_id: "com.example.calendar".into(),
        revision_id: Some(revision_id.into()),
    }
}
