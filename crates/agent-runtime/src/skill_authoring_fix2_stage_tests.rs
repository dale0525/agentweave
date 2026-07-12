use crate::skill_authoring_tests::{AuthoringFixture, write_package};
use crate::skill_management::{CreateSkillDraftRequest, DraftFileUpdate};
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant};
use crate::skill_state::{SkillApprovalStatus, SkillRevisionStatus};
use crate::skill_store::{SkillStoreFaultPoint, SkillStoreTestFaults};
use std::path::Path;

#[tokio::test]
async fn detached_import_completes_at_every_transfer_stage_after_waiter_abort() {
    for point in [
        SkillStoreFaultPoint::ImportAfterReserve,
        SkillStoreFaultPoint::ImportAfterCopy,
        SkillStoreFaultPoint::ImportBeforeRow,
        SkillStoreFaultPoint::ImportAfterRow,
        SkillStoreFaultPoint::ImportBeforeFinalize,
    ] {
        let faults = SkillStoreTestFaults::default();
        let fixture = AuthoringFixture::with_faults(faults.clone()).await;
        write_package(
            &fixture.imports.path().join("stage-import"),
            "com.example.stage-import",
            SkillPackageKind::InstructionOnly,
        )
        .await;
        let stage = faults.gate_once(point);
        let terminal = faults.gate_once(SkillStoreFaultPoint::ImportTerminal);
        let service = fixture.service.clone();
        let actor = fixture.actor([SkillGrant::Import]);
        let waiter = tokio::spawn(async move {
            service
                .import_draft(&actor, Path::new("stage-import"))
                .await
        });
        stage.wait_entered().await;

        waiter.abort();
        stage.release().await;
        terminal.wait_entered().await;

        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT revision_id, lifecycle_status, storage_path FROM skill_revisions",
        )
        .fetch_all(fixture.state.pool())
        .await
        .unwrap();
        assert_eq!(rows.len(), 1, "stage {point:?}");
        assert_eq!(rows[0].1, SkillRevisionStatus::Quarantined.as_str());
        assert!(Path::new(&rows[0].2).is_dir());
        assert!(
            fixture
                .state
                .get_installation(
                    &crate::skill_package::SkillPackageId::parse("com.example.stage-import")
                        .unwrap(),
                )
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(fixture.manager.current_snapshot().generation(), 1);
        terminal.release().await;
    }
}

#[tokio::test]
async fn import_pre_row_and_cleanup_failures_leave_no_row_or_partial_tree() {
    let faults = SkillStoreTestFaults::default();
    faults.fail_once(SkillStoreFaultPoint::ImportBeforeRow);
    faults.fail_once(SkillStoreFaultPoint::TransferCleanup);
    let fixture = AuthoringFixture::with_faults(faults).await;
    write_package(
        &fixture.imports.path().join("failed-import"),
        "com.example.failed-import",
        SkillPackageKind::InstructionOnly,
    )
    .await;

    let error = fixture
        .service
        .import_draft(
            &fixture.actor([SkillGrant::Import]),
            Path::new("failed-import"),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Internal { .. })
    ));
    let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_revisions")
        .fetch_one(fixture.state.pool())
        .await
        .unwrap();
    assert_eq!(row_count, 0);
    assert_eq!(
        directory_entry_count(&fixture.store.paths().quarantine).await,
        0
    );
    assert_eq!(fixture.manager.current_snapshot().generation(), 1);
}

#[tokio::test]
async fn activation_stage_gates_prove_durable_memory_event_cleanup_order() {
    let faults = SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let draft = fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse("com.example.activation-stages").unwrap(),
                display_name: "Activation stages".into(),
                description: "Activation stage verification.".into(),
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
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    let staging = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    let before_commit = faults.gate_once(SkillStoreFaultPoint::ActivationBeforeDurableCommit);
    let after_commit = faults.gate_once(SkillStoreFaultPoint::ActivationAfterDurableCommit);
    let after_memory = faults.gate_once(SkillStoreFaultPoint::ActivationAfterMemoryPublish);
    let after_event = faults.gate_once(SkillStoreFaultPoint::ActivationAfterEvent);
    let after_cleanup = faults.gate_once(SkillStoreFaultPoint::ActivationAfterSourceCleanup);
    let mut events = fixture.service.subscribe_events();
    let service = fixture.service.clone();
    let approval_id = approval.approval_id.clone();
    let waiter = tokio::spawn(async move {
        service
            .approve_activation(
                &approval_id,
                &ActorContext::owner("approver-2", [SkillGrant::Activate]),
            )
            .await
    });

    before_commit.wait_entered().await;
    assert_eq!(fixture.manager.current_snapshot().generation(), 1);
    assert_eq!(
        fixture
            .state
            .get_revision(&draft.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Staging
    );
    assert!(Path::new(&staging.storage_path).is_dir());
    before_commit.release().await;

    after_commit.wait_entered().await;
    let managed = fixture
        .state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(managed.status, SkillRevisionStatus::Managed);
    assert_eq!(
        fixture
            .state
            .get_approval(&approval.approval_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillApprovalStatus::Approved
    );
    assert!(
        fixture
            .state
            .get_installation(&draft.package_id)
            .await
            .unwrap()
            .is_some()
    );
    assert_eq!(fixture.manager.current_snapshot().generation(), 1);
    assert!(events.try_recv().is_err());
    after_commit.release().await;

    after_memory.wait_entered().await;
    assert_eq!(fixture.manager.current_snapshot().generation(), 2);
    assert!(events.try_recv().is_err());
    after_memory.release().await;

    after_event.wait_entered().await;
    assert!(matches!(
        events.recv().await.unwrap(),
        crate::events::RuntimeEvent::SkillSnapshotPublished { generation: 2 }
    ));
    after_event.release().await;

    after_cleanup.wait_entered().await;
    assert!(!Path::new(&staging.storage_path).exists());
    assert!(Path::new(&managed.storage_path).is_dir());
    after_cleanup.release().await;
    let report = waiter.await.unwrap().unwrap();
    assert_eq!(report.active_generation, 2);
}

#[tokio::test]
async fn draft_test_negative_matrix_persists_stable_classes_without_publication() {
    let fixture = AuthoringFixture::new().await;
    let missing_dependency =
        create_instruction_draft(&fixture, "com.example.missing-dependency").await;
    update_descriptor(&fixture, &missing_dependency.revision_id, |descriptor| {
        descriptor["requires"]["packages"] = serde_json::json!(["com.example.not-installed"]);
    })
    .await;
    assert_test_class(
        &fixture,
        &missing_dependency.revision_id,
        "resolver_inactive",
    )
    .await;

    let missing_capability =
        create_instruction_draft(&fixture, "com.example.missing-capability").await;
    update_descriptor(&fixture, &missing_capability.revision_id, |descriptor| {
        descriptor["requires"]["capabilities"] = serde_json::json!(["network"]);
    })
    .await;
    assert_test_class(
        &fixture,
        &missing_capability.revision_id,
        "forbidden_capability",
    )
    .await;

    let tool_fixture = AuthoringFixture::with_known_runtime_tool().await;
    let tool_draft = tool_fixture
        .service
        .create_draft(
            &tool_fixture.actor([SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse("com.example.unknown-test-tool").unwrap(),
                display_name: "Unknown tool".into(),
                description: "Unknown tool test.".into(),
                kind: SkillPackageKind::HostToolsOnly,
                required_tools: vec!["calendar_create".into()],
            },
        )
        .await
        .unwrap();
    update_descriptor(&tool_fixture, &tool_draft.revision_id, |descriptor| {
        descriptor["requires"]["runtimeTools"] = serde_json::json!(["missing_runtime_tool"]);
    })
    .await;
    assert_test_class(&tool_fixture, &tool_draft.revision_id, "unknown_tool").await;

    let connector_fixture = AuthoringFixture::with_connectors(["com.example.calendar"]).await;
    write_package(
        &connector_fixture.imports.path().join("connector-test"),
        "com.example.connector-test",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let descriptor_path = connector_fixture
        .imports
        .path()
        .join("connector-test/general-agent.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
    descriptor["kind"] = serde_json::json!("host_tools_only");
    descriptor["requires"]["connectors"] = serde_json::json!(["com.example.calendar"]);
    tokio::fs::write(
        &descriptor_path,
        format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
    )
    .await
    .unwrap();
    let connector_draft = connector_fixture
        .service
        .import_draft(
            &connector_fixture.actor([SkillGrant::Import]),
            Path::new("connector-test"),
        )
        .await
        .unwrap();
    connector_fixture
        .service
        .validate_draft(
            &connector_fixture.actor([SkillGrant::Validate]),
            &connector_draft.revision_id,
        )
        .await
        .unwrap();
    update_descriptor(
        &connector_fixture,
        &connector_draft.revision_id,
        |descriptor| {
            descriptor["requires"]["connectors"] = serde_json::json!(["com.example.missing"]);
        },
    )
    .await;
    assert_test_class(
        &connector_fixture,
        &connector_draft.revision_id,
        "unknown_connector",
    )
    .await;

    update_descriptor(
        &connector_fixture,
        &connector_draft.revision_id,
        |descriptor| {
            descriptor["requires"]["connectors"] = serde_json::json!([" invalid/connector "]);
        },
    )
    .await;
    assert_test_class(
        &connector_fixture,
        &connector_draft.revision_id,
        "validation_failed",
    )
    .await;
}

#[tokio::test]
async fn activation_request_before_commit_abort_still_emits_exactly_once() {
    let faults = SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let draft = create_instruction_draft(&fixture, "com.example.request-before-commit").await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let before = faults.gate_once(SkillStoreFaultPoint::ActivationRequestBeforeCommit);
    let after = faults.gate_once(SkillStoreFaultPoint::ActivationRequestAfterCommit);
    let mut events = fixture.service.subscribe_events();
    let service = fixture.service.clone();
    let actor = fixture.actor([SkillGrant::Activate]);
    let revision_id = draft.revision_id.clone();
    let waiter =
        tokio::spawn(async move { service.request_activation(&actor, &revision_id).await });
    before.wait_entered().await;

    waiter.abort();
    before.release().await;
    after.wait_entered().await;
    after.release().await;

    assert!(matches!(
        events.recv().await.unwrap(),
        crate::events::RuntimeEvent::SkillApprovalRequired { revision_id, .. }
            if revision_id == draft.revision_id
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
    let retry = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    assert_eq!(retry.status, SkillApprovalStatus::Pending);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn host_connector_catalog_uses_the_same_canonical_parser() {
    let fixture = AuthoringFixture::new().await;
    for connectors in [
        vec![""],
        vec![" Calendar "],
        vec!["invalid/connector"],
        vec!["calendar", "CALENDAR"],
    ] {
        let error = fixture
            .service
            .clone()
            .with_connector_catalog(connectors)
            .err()
            .unwrap();
        assert!(matches!(
            error.downcast_ref::<crate::skill_management::SkillManagementError>(),
            Some(crate::skill_management::SkillManagementError::InvalidRequest(_))
        ));
    }
    fixture
        .service
        .clone()
        .with_connector_catalog(["calendar", "com.example.storage"])
        .unwrap();
}

#[tokio::test]
async fn revision_lock_edit_race_makes_bound_approval_conflict_without_publication() {
    let faults = SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let draft = create_instruction_draft(&fixture, "com.example.revision-lock-race").await;
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
    let lock_attempt = faults.gate_once(SkillStoreFaultPoint::RevisionLockAttempt);
    let service = fixture.service.clone();
    let approval_id = approval.approval_id.clone();
    let approver = ActorContext::owner("approver-2", [SkillGrant::Activate]);
    let waiter =
        tokio::spawn(async move { service.approve_activation(&approval_id, &approver).await });
    lock_attempt.wait_entered().await;

    fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![DraftFileUpdate {
                path: "SKILL.md".into(),
                content: "# Edited while approval waits\n".into(),
            }],
        )
        .await
        .unwrap();
    lock_attempt.release().await;
    let error = waiter.await.unwrap().unwrap_err();

    assert!(matches!(
        error.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
    assert_eq!(
        fixture
            .state
            .get_approval(&approval.approval_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillApprovalStatus::Pending
    );
    assert_eq!(
        fixture
            .state
            .get_revision(&draft.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillRevisionStatus::Staging
    );
    assert_eq!(fixture.manager.current_snapshot().generation(), 1);
    assert!(
        fixture
            .state
            .get_installation(&draft.package_id)
            .await
            .unwrap()
            .is_none()
    );
}

async fn create_instruction_draft(
    fixture: &AuthoringFixture,
    package_id: &str,
) -> crate::skill_management::SkillDraftSummary {
    fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse(package_id).unwrap(),
                display_name: "Draft test".into(),
                description: "Draft test matrix.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap()
}

async fn update_descriptor(
    fixture: &AuthoringFixture,
    revision_id: &str,
    update: impl FnOnce(&mut serde_json::Value),
) {
    let record = fixture
        .state
        .get_revision(revision_id)
        .await
        .unwrap()
        .unwrap();
    let path = Path::new(&record.storage_path).join("general-agent.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap();
    update(&mut descriptor);
    fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            revision_id,
            vec![DraftFileUpdate {
                path: "general-agent.json".into(),
                content: format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
            }],
        )
        .await
        .unwrap();
}

async fn assert_test_class(fixture: &AuthoringFixture, revision_id: &str, expected: &str) {
    let validation = fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), revision_id)
        .await
        .unwrap();
    assert!(!validation.ok);
    let generation = fixture.manager.current_snapshot().generation();
    let result = fixture
        .service
        .test_draft(&fixture.actor([SkillGrant::Test]), revision_id)
        .await
        .unwrap();
    assert!(!result.ok);
    assert_eq!(result.error_class.as_deref(), Some(expected));
    let persisted = fixture
        .state
        .revision_validation(revision_id)
        .await
        .unwrap();
    assert_eq!(persisted["test"]["errorClass"], expected);
    assert_eq!(fixture.manager.current_snapshot().generation(), generation);
    let installations: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_installations")
        .fetch_one(fixture.state.pool())
        .await
        .unwrap();
    assert_eq!(installations, 0);
}

async fn directory_entry_count(root: &Path) -> usize {
    let mut entries = tokio::fs::read_dir(root).await.unwrap();
    let mut count = 0;
    while entries.next_entry().await.unwrap().is_some() {
        count += 1;
    }
    count
}
