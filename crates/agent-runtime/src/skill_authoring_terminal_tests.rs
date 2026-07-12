use crate::skill_authoring_tests::{AuthoringFixture, write_package};
use crate::skill_management::{CreateSkillDraftRequest, SkillManagementError};
use crate::skill_management_tools::{SkillManagementToolContext, SkillManagementTools};
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant};
use crate::skill_state::SkillRevisionStatus;
use crate::skill_store::{SkillStoreFaultPoint, SkillStoreTestFaults};
use crate::skill_store_public_types::SkillStoreBoundaryError;
use serde_json::json;
use std::path::Path;

#[tokio::test]
async fn quarantine_release_missing_and_invalid_lifecycle_are_typed() {
    let missing_fixture = AuthoringFixture::new().await;
    let missing = import_quarantined(
        &missing_fixture,
        "missing-release",
        "com.example.missing-release",
    )
    .await;
    let missing_record = revision(&missing_fixture, &missing.revision_id).await;
    sqlx::query("DELETE FROM skill_revisions WHERE revision_id = ?")
        .bind(&missing.revision_id)
        .execute(missing_fixture.state.pool())
        .await
        .unwrap();
    let missing_error = missing_fixture
        .store
        .release_quarantined_revision(missing_record, json!({"ok": true}))
        .await
        .unwrap_err();
    assert!(matches!(
        missing_error.downcast_ref::<SkillStoreBoundaryError>(),
        Some(SkillStoreBoundaryError::NotFound(_))
    ));
    assert_safe_error(&missing_error, &missing_fixture, &missing.revision_id);

    let lifecycle_fixture = AuthoringFixture::new().await;
    let lifecycle = import_quarantined(
        &lifecycle_fixture,
        "lifecycle-release",
        "com.example.lifecycle-release",
    )
    .await;
    let mut lifecycle_record = revision(&lifecycle_fixture, &lifecycle.revision_id).await;
    lifecycle_record.status = SkillRevisionStatus::Staging;
    let lifecycle_error = lifecycle_fixture
        .store
        .release_quarantined_revision(lifecycle_record, json!({"ok": true}))
        .await
        .unwrap_err();
    assert!(matches!(
        lifecycle_error.downcast_ref::<SkillStoreBoundaryError>(),
        Some(SkillStoreBoundaryError::Conflict(_))
    ));
    assert_safe_error(&lifecycle_error, &lifecycle_fixture, &lifecycle.revision_id);
}

#[tokio::test]
async fn concurrent_quarantine_validation_has_one_staging_winner_and_typed_loser() {
    let faults = SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let imported = import_quarantined(
        &fixture,
        "service-validation-race",
        "com.example.service-validation-race",
    )
    .await;
    let gate = faults.gate_once(SkillStoreFaultPoint::ValidateDraftBeforePersist);
    let service = fixture.service.clone();
    let actor = fixture.actor([SkillGrant::Validate]);
    let revision_id = imported.revision_id.clone();
    let loser = tokio::spawn(async move { service.validate_draft(&actor, &revision_id).await });
    gate.wait_entered().await;

    let winner = fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &imported.revision_id,
        )
        .await
        .unwrap();
    gate.release().await;
    let error = loser.await.unwrap().unwrap_err();

    assert!(winner.ok);
    assert_management_conflict(&error);
    assert_safe_error(&error, &fixture, &imported.revision_id);
    assert_eq!(
        revision(&fixture, &imported.revision_id).await.status,
        SkillRevisionStatus::Staging
    );
}

#[tokio::test]
async fn model_quarantine_validation_loser_has_stable_conflict_code() {
    let faults = SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let imported = import_quarantined(
        &fixture,
        "model-validation-race",
        "com.example.model-validation-race",
    )
    .await;
    let gate = faults.gate_once(SkillStoreFaultPoint::ValidateDraftBeforePersist);
    let context = SkillManagementToolContext {
        service: fixture.service.clone(),
        actor: fixture.actor([SkillGrant::Validate]),
    };
    let revision_id = imported.revision_id.clone();
    let loser = tokio::spawn(async move {
        SkillManagementTools::execute(
            &context,
            "validate_skill_draft",
            "quarantine-validation-race",
            json!({"revision_id": revision_id}),
        )
        .await
    });
    gate.wait_entered().await;

    fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &imported.revision_id,
        )
        .await
        .unwrap();
    gate.release().await;
    let result = loser.await.unwrap();

    assert!(!result.ok);
    let error = result.error.unwrap();
    assert_eq!(error.code, "conflict");
    assert_safe_text(&error.message, &fixture, &imported.revision_id);
    assert_eq!(
        revision(&fixture, &imported.revision_id).await.status,
        SkillRevisionStatus::Staging
    );
}

#[tokio::test]
async fn activation_then_ordinary_reload_allows_a_new_approval_request() {
    let fixture = AuthoringFixture::new().await;
    activate_package(&fixture, "com.example.first-activation").await;
    let reload = fixture.manager.reload().await.unwrap();
    assert_eq!(reload.active_generation, 3);

    let draft = create_validated_draft(&fixture, "com.example.after-reload").await;
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();

    assert_eq!(approval.revision_id, draft.revision_id);
    assert_eq!(fixture.manager.current_snapshot().generation(), 3);
    let durable: i64 =
        sqlx::query_scalar("SELECT generation FROM skill_snapshots WHERE status = 'active'")
            .fetch_one(fixture.state.pool())
            .await
            .unwrap();
    assert_eq!(durable, 2);
}

#[tokio::test]
async fn durable_generation_ahead_of_publication_guard_conflicts_without_request() {
    let faults = SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let draft = create_validated_draft(&fixture, "com.example.durable-ahead").await;
    let gate = faults.gate_once(SkillStoreFaultPoint::ActivationRequestBeforeCommit);
    let service = fixture.service.clone();
    let actor = fixture.actor([SkillGrant::Activate]);
    let revision_id = draft.revision_id.clone();
    let request =
        tokio::spawn(async move { service.request_activation(&actor, &revision_id).await });
    gate.wait_entered().await;
    fixture
        .state
        .record_snapshot_candidate(2, json!([]))
        .await
        .unwrap();
    fixture.state.record_snapshot_activation(2).await.unwrap();
    gate.release().await;

    let error = request.await.unwrap().unwrap_err();
    assert_management_conflict(&error);
    assert_safe_error(&error, &fixture, &draft.revision_id);
    let approvals: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM skill_approvals WHERE revision_id = ?")
            .bind(&draft.revision_id)
            .fetch_one(fixture.state.pool())
            .await
            .unwrap();
    assert_eq!(approvals, 0);
}

async fn import_quarantined(
    fixture: &AuthoringFixture,
    import_name: &str,
    package_id: &str,
) -> crate::skill_management::SkillDraftSummary {
    write_package(
        &fixture.imports.path().join(import_name),
        package_id,
        SkillPackageKind::InstructionOnly,
    )
    .await;
    fixture
        .service
        .import_draft(&fixture.actor([SkillGrant::Import]), Path::new(import_name))
        .await
        .unwrap()
}

async fn create_validated_draft(
    fixture: &AuthoringFixture,
    package_id: &str,
) -> crate::skill_management::SkillDraftSummary {
    let draft = fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse(package_id).unwrap(),
                display_name: "Terminal gate".into(),
                description: "Terminal gate package.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    draft
}

async fn activate_package(fixture: &AuthoringFixture, package_id: &str) {
    let draft = create_validated_draft(fixture, package_id).await;
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    fixture
        .service
        .approve_activation(
            &approval.approval_id,
            &ActorContext::owner("approver-terminal", [SkillGrant::Activate]),
        )
        .await
        .unwrap();
}

async fn revision(
    fixture: &AuthoringFixture,
    revision_id: &str,
) -> crate::skill_state::SkillRevisionRecord {
    fixture
        .state
        .get_revision(revision_id)
        .await
        .unwrap()
        .unwrap()
}

fn assert_management_conflict(error: &anyhow::Error) {
    assert!(matches!(
        error.downcast_ref::<SkillManagementError>(),
        Some(SkillManagementError::Conflict { .. })
    ));
}

fn assert_safe_error(error: &anyhow::Error, fixture: &AuthoringFixture, revision_id: &str) {
    assert_safe_text(&error.to_string(), fixture, revision_id);
}

fn assert_safe_text(text: &str, fixture: &AuthoringFixture, revision_id: &str) {
    assert!(!text.contains(revision_id));
    assert!(!text.contains("skill_revisions"));
    assert!(!text.contains("secret-token"));
    assert!(
        !text.contains(
            fixture
                .store
                .paths()
                .managed
                .parent()
                .unwrap()
                .to_str()
                .unwrap()
        )
    );
}
