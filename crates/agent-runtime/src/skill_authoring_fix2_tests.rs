use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_authoring_tests::write_package;
use crate::skill_management::{CreateSkillDraftRequest, DraftFileUpdate, SkillManagementError};
use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{ActorContext, SkillGrant};
use crate::skill_state::{SkillInstallStatus, SkillLayerRecord, SkillRevisionStatus};
use std::path::Path;

#[tokio::test]
async fn validation_preview_does_not_quarantine_corrupt_managed_peer() {
    let fixture = AuthoringFixture::new().await;
    let active = activate_package(&fixture, "com.example.active-peer").await;
    let active_record = fixture
        .state
        .get_revision(&active.revision_id)
        .await
        .unwrap()
        .unwrap();
    let active_installation = fixture
        .state
        .get_installation(&active.package_id)
        .await
        .unwrap()
        .unwrap();
    let generation = fixture.manager.current_snapshot().generation();
    let initial_snapshot_rows = snapshot_rows(&fixture).await;
    let initial_audit_count = audit_count(&fixture).await;
    let mut events = fixture.service.subscribe_events();

    corrupt_managed_instructions(Path::new(&active_record.storage_path)).await;
    let candidate = create_package(&fixture, "com.example.preview-candidate").await;
    let validation = fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &candidate.revision_id,
        )
        .await
        .unwrap();

    assert!(!validation.ok);
    assert_eq!(
        fixture
            .state
            .get_revision(&active.revision_id)
            .await
            .unwrap()
            .unwrap(),
        active_record
    );
    assert_eq!(
        fixture
            .state
            .get_installation(&active.package_id)
            .await
            .unwrap()
            .unwrap(),
        active_installation
    );
    assert_eq!(active_record.status, SkillRevisionStatus::Managed);
    assert_eq!(active_installation.status, SkillInstallStatus::Active);
    assert!(Path::new(&active_record.storage_path).is_dir());
    assert_eq!(fixture.manager.current_snapshot().generation(), generation);
    assert_eq!(snapshot_rows(&fixture).await, initial_snapshot_rows);
    assert_eq!(audit_count(&fixture).await, initial_audit_count);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn draft_test_rejects_validation_from_same_hash_different_revision() {
    let fixture = AuthoringFixture::new().await;
    let first = create_package(&fixture, "com.example.same-bytes").await;
    let validation = fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &first.revision_id)
        .await
        .unwrap();
    assert!(validation.ok);
    let second = create_package(&fixture, "com.example.same-bytes").await;
    let first_record = fixture
        .state
        .get_revision(&first.revision_id)
        .await
        .unwrap()
        .unwrap();
    let second_record = fixture
        .state
        .get_revision(&second.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first_record.content_hash, second_record.content_hash);
    sqlx::query("UPDATE skill_revisions SET validation_json = ? WHERE revision_id = ?")
        .bind(first_record.validation_json.to_string())
        .bind(&second.revision_id)
        .execute(fixture.state.pool())
        .await
        .unwrap();

    let error = fixture
        .service
        .test_draft(&fixture.actor([SkillGrant::Test]), &second.revision_id)
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
}

#[tokio::test]
async fn connector_validation_rejects_noncanonical_invalid_and_duplicate_ids() {
    let fixture = AuthoringFixture::with_connectors(["com.example.calendar"]).await;
    let package_root = fixture.imports.path().join("invalid-connectors");
    write_package(
        &package_root,
        "com.example.invalid-connectors",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let descriptor_path = package_root.join("agentweave.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
    descriptor["kind"] = serde_json::json!("host_tools_only");
    descriptor["requires"]["connectors"] = serde_json::json!([
        "com.example.calendar",
        " COM.EXAMPLE.CALENDAR ",
        "invalid/connector",
        ""
    ]);
    tokio::fs::write(
        descriptor_path,
        format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
    )
    .await
    .unwrap();
    let imported = fixture
        .service
        .import_draft(
            &fixture.actor([SkillGrant::Import]),
            Path::new("invalid-connectors"),
        )
        .await
        .unwrap();

    let validation = fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &imported.revision_id,
        )
        .await
        .unwrap();

    assert!(!validation.ok);
    assert!(
        validation
            .errors
            .contains(&"invalid required connector: connector id must not be empty".into())
    );
    assert!(validation.errors.contains(
        &"invalid required connector: connector id must use canonical lowercase ASCII".into()
    ));
    assert!(
        validation.errors.contains(
            &"invalid required connector: connector id contains invalid characters".into()
        )
    );
    assert!(validation.errors.contains(
        &"invalid required connector: duplicate connector id after normalization".into()
    ));
    let persisted = fixture
        .state
        .revision_validation(&imported.revision_id)
        .await
        .unwrap();
    assert_eq!(persisted["ok"], false);
}

#[tokio::test]
async fn unknown_tool_import_is_quarantined_then_fails_persisted_validation() {
    let fixture = AuthoringFixture::new().await;
    let package_root = fixture.imports.path().join("unknown-tool");
    write_package(
        &package_root,
        "com.example.unknown-tool-import",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let descriptor_path = package_root.join("agentweave.json");
    let mut descriptor: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&descriptor_path).await.unwrap()).unwrap();
    descriptor["kind"] = serde_json::json!("host_tools_only");
    descriptor["requires"]["runtimeTools"] = serde_json::json!(["missing_runtime_tool"]);
    tokio::fs::write(
        descriptor_path,
        format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
    )
    .await
    .unwrap();
    let generation = fixture.manager.current_snapshot().generation();
    let initial_snapshot_rows = snapshot_rows(&fixture).await;
    let mut events = fixture.service.subscribe_events();

    let imported = fixture
        .service
        .import_draft(
            &fixture.actor([SkillGrant::Import]),
            Path::new("unknown-tool"),
        )
        .await
        .unwrap();
    let quarantined = fixture
        .state
        .get_revision(&imported.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(quarantined.status, SkillRevisionStatus::Quarantined);
    assert!(Path::new(&quarantined.storage_path).is_dir());

    let validation = fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &imported.revision_id,
        )
        .await
        .unwrap();

    assert!(!validation.ok);
    assert!(
        validation
            .errors
            .contains(&"unknown required host tool: missing_runtime_tool".into())
    );
    let retained = fixture
        .state
        .get_revision(&imported.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(retained.status, SkillRevisionStatus::Quarantined);
    assert_eq!(retained.validation_json["ok"], false);
    assert!(
        fixture
            .state
            .get_installation(&imported.package_id)
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(fixture.manager.current_snapshot().generation(), generation);
    assert_eq!(snapshot_rows(&fixture).await, initial_snapshot_rows);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn exact_import_replay_reports_authoritative_staging_validation() {
    let fixture = AuthoringFixture::new().await;
    write_package(
        &fixture.imports.path().join("replay-source"),
        "com.example.import-replay-truth",
        SkillPackageKind::InstructionOnly,
    )
    .await;
    let actor = fixture.actor([SkillGrant::Import]);
    let first = fixture
        .service
        .import_draft(&actor, Path::new("replay-source"))
        .await
        .unwrap();
    let validation = fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &first.revision_id)
        .await
        .unwrap();
    assert!(validation.ok, "{:?}", validation.errors);
    let authoritative = fixture
        .state
        .get_revision(&first.revision_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(authoritative.status, SkillRevisionStatus::Staging);
    let row_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_revisions")
        .fetch_one(fixture.state.pool())
        .await
        .unwrap();
    let staging_entries = directory_entry_count(fixture.store.paths().staging.as_path()).await;
    let quarantine_entries =
        directory_entry_count(fixture.store.paths().quarantine.as_path()).await;

    let replay = fixture
        .service
        .import_draft(&actor, Path::new("replay-source"))
        .await
        .unwrap();

    assert_eq!(replay.revision_id, first.revision_id);
    assert_eq!(replay.status, "staging");
    assert_eq!(replay.version, authoritative.version);
    assert_eq!(replay.kind, SkillPackageKind::InstructionOnly);
    assert_eq!(replay.validation, authoritative.validation_json);
    assert_eq!(replay.validation["ok"], true);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM skill_revisions")
            .fetch_one(fixture.state.pool())
            .await
            .unwrap(),
        row_count
    );
    assert_eq!(
        directory_entry_count(fixture.store.paths().staging.as_path()).await,
        staging_entries
    );
    assert_eq!(
        directory_entry_count(fixture.store.paths().quarantine.as_path()).await,
        quarantine_entries
    );
}

#[tokio::test]
async fn typed_boundaries_map_malformed_update_to_invalid_request() {
    let fixture = AuthoringFixture::new().await;
    let draft = create_package(&fixture, "com.example.typed-update").await;

    let error = fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![DraftFileUpdate {
                path: "agentweave.json".into(),
                content: "{ malformed descriptor".into(),
            }],
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillManagementError>(),
        Some(SkillManagementError::InvalidRequest(_))
    ));
    let public = error.to_string();
    assert!(!public.contains("malformed descriptor"));
    assert!(!public.contains("staging"));
}

#[tokio::test]
async fn typed_boundaries_map_wrong_draft_lifecycle_to_conflict() {
    let fixture = AuthoringFixture::new().await;
    let draft = create_package(&fixture, "com.example.typed-lifecycle").await;
    fixture
        .store
        .promote_revision(&draft.revision_id)
        .await
        .unwrap();

    let validate = fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap_err();
    let test = fixture
        .service
        .test_draft(&fixture.actor([SkillGrant::Test]), &draft.revision_id)
        .await
        .unwrap_err();
    let request = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap_err();

    for error in [validate, test, request] {
        assert!(matches!(
            error.downcast_ref::<SkillManagementError>(),
            Some(SkillManagementError::Conflict { .. })
        ));
        assert!(!error.to_string().contains(&draft.revision_id));
    }
}

#[tokio::test]
async fn typed_boundaries_map_import_parser_and_missing_source() {
    let fixture = AuthoringFixture::new().await;
    let malformed = fixture.imports.path().join("malformed-import");
    tokio::fs::create_dir_all(&malformed).await.unwrap();
    tokio::fs::write(malformed.join("agentweave.json"), b"{ not json")
        .await
        .unwrap();
    tokio::fs::write(malformed.join("SKILL.md"), b"# Imported")
        .await
        .unwrap();
    let actor = fixture.actor([SkillGrant::Import]);

    let invalid = fixture
        .service
        .import_draft(&actor, Path::new("malformed-import"))
        .await
        .unwrap_err();
    let missing = fixture
        .service
        .import_draft(&actor, Path::new("missing-import"))
        .await
        .unwrap_err();

    assert!(matches!(
        invalid.downcast_ref::<SkillManagementError>(),
        Some(SkillManagementError::InvalidRequest(_))
    ));
    assert!(matches!(
        missing.downcast_ref::<SkillManagementError>(),
        Some(SkillManagementError::NotFound { .. })
    ));
    for error in [invalid, missing] {
        let public = error.to_string();
        assert!(!public.contains(fixture.imports.path().to_str().unwrap()));
        assert!(!public.contains("column"));
    }
}

#[tokio::test]
async fn typed_boundaries_map_unverified_export_to_conflict() {
    let fixture = AuthoringFixture::new().await;
    let draft = create_package(&fixture, "com.example.unverified-export").await;
    let promoted = fixture
        .store
        .promote_revision(&draft.revision_id)
        .await
        .unwrap();
    fixture
        .state
        .activate_revision(
            &draft.package_id,
            &promoted.revision_id,
            SkillLayerRecord::Managed,
            "approver-2",
        )
        .await
        .unwrap();

    let error = fixture
        .service
        .export_managed_skill(
            &fixture.actor([SkillGrant::Export]),
            &draft.package_id,
            Path::new("unverified"),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<SkillManagementError>(),
        Some(SkillManagementError::Conflict { .. })
    ));
    assert!(!error.to_string().contains(&promoted.revision_id));
}

#[tokio::test]
async fn full_draft_test_deadline_covers_pre_snapshot_wait() {
    let faults = crate::skill_store::SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let draft = create_package(&fixture, "com.example.test-deadline").await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let gate = faults.gate_once(crate::skill_store::SkillStoreFaultPoint::DraftTestBeforeSnapshot);
    let service = fixture
        .service
        .clone()
        .with_draft_test_deadline(std::time::Duration::from_millis(40));
    let actor = fixture.actor([SkillGrant::Test]);
    let revision_id = draft.revision_id.clone();
    let waiter = tokio::spawn(async move { service.test_draft(&actor, &revision_id).await });
    gate.wait_entered().await;
    let result = tokio::time::timeout(std::time::Duration::from_millis(200), waiter)
        .await
        .expect("draft test operation must honor its own deadline")
        .unwrap()
        .unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_class.as_deref(), Some("timeout"));
    let persisted = fixture
        .state
        .revision_validation(&draft.revision_id)
        .await
        .unwrap();
    assert_eq!(persisted["test"]["errorClass"], "timeout");
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

#[tokio::test]
async fn activation_request_event_survives_waiter_abort_after_commit() {
    let faults = crate::skill_store::SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let draft = create_package(&fixture, "com.example.detached-request").await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let gate =
        faults.gate_once(crate::skill_store::SkillStoreFaultPoint::ActivationRequestAfterCommit);
    let mut events = fixture.service.subscribe_events();
    let service = fixture.service.clone();
    let actor = fixture.actor([SkillGrant::Activate]);
    let revision_id = draft.revision_id.clone();
    let waiter =
        tokio::spawn(async move { service.request_activation(&actor, &revision_id).await });
    gate.wait_entered().await;

    waiter.abort();
    gate.release().await;

    let event = tokio::time::timeout(std::time::Duration::from_millis(200), events.recv())
        .await
        .expect("committed approval must emit an event")
        .unwrap();
    assert!(matches!(
        event,
        crate::events::RuntimeEvent::SkillApprovalRequired { revision_id, .. }
            if revision_id == draft.revision_id
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
    let approvals: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM skill_approvals")
        .fetch_one(fixture.state.pool())
        .await
        .unwrap();
    assert_eq!(approvals, 1);
}

#[tokio::test]
async fn draft_test_terminal_persistence_survives_waiter_abort() {
    let faults = crate::skill_store::SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let draft = create_package(&fixture, "com.example.detached-test").await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let before = faults.gate_once(crate::skill_store::SkillStoreFaultPoint::DraftTestBeforePersist);
    let after = faults.gate_once(crate::skill_store::SkillStoreFaultPoint::DraftTestAfterPersist);
    let service = fixture.service.clone();
    let actor = fixture.actor([SkillGrant::Test]);
    let revision_id = draft.revision_id.clone();
    let waiter = tokio::spawn(async move { service.test_draft(&actor, &revision_id).await });
    before.wait_entered().await;

    waiter.abort();
    before.release().await;
    after.wait_entered().await;

    let persisted = fixture
        .state
        .revision_validation(&draft.revision_id)
        .await
        .unwrap();
    assert_eq!(persisted["test"]["ok"], true);
    assert_eq!(fixture.manager.current_snapshot().generation(), 1);
    assert!(
        fixture
            .state
            .get_installation(&draft.package_id)
            .await
            .unwrap()
            .is_none()
    );
    after.release().await;
}

#[tokio::test]
async fn draft_test_preview_does_not_quarantine_corrupt_managed_peer() {
    let fixture = AuthoringFixture::new().await;
    let active = activate_package(&fixture, "com.example.test-active-peer").await;
    let candidate = create_package(&fixture, "com.example.test-preview-candidate").await;
    fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &candidate.revision_id,
        )
        .await
        .unwrap();
    let active_record = fixture
        .state
        .get_revision(&active.revision_id)
        .await
        .unwrap()
        .unwrap();
    let active_installation = fixture
        .state
        .get_installation(&active.package_id)
        .await
        .unwrap()
        .unwrap();
    let generation = fixture.manager.current_snapshot().generation();
    let initial_snapshot_rows = snapshot_rows(&fixture).await;
    let initial_audit_count = audit_count(&fixture).await;
    let mut events = fixture.service.subscribe_events();
    corrupt_managed_instructions(Path::new(&active_record.storage_path)).await;

    let result = fixture
        .service
        .test_draft(&fixture.actor([SkillGrant::Test]), &candidate.revision_id)
        .await
        .unwrap();

    assert!(!result.ok);
    assert_eq!(result.error_class.as_deref(), Some("resolver_inactive"));
    assert_eq!(
        fixture
            .state
            .get_revision(&active.revision_id)
            .await
            .unwrap()
            .unwrap(),
        active_record
    );
    assert_eq!(
        fixture
            .state
            .get_installation(&active.package_id)
            .await
            .unwrap()
            .unwrap(),
        active_installation
    );
    assert!(Path::new(&active_record.storage_path).is_dir());
    assert_eq!(fixture.manager.current_snapshot().generation(), generation);
    assert_eq!(snapshot_rows(&fixture).await, initial_snapshot_rows);
    assert_eq!(audit_count(&fixture).await, initial_audit_count);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn cancelled_activation_build_failure_preserves_corrupt_managed_peer() {
    let faults = crate::skill_store::SkillStoreTestFaults::default();
    let fixture = AuthoringFixture::with_faults(faults.clone()).await;
    let active = activate_package(&fixture, "com.example.activation-active-peer").await;
    let candidate = create_package(&fixture, "com.example.activation-candidate").await;
    fixture
        .service
        .validate_draft(
            &fixture.actor([SkillGrant::Validate]),
            &candidate.revision_id,
        )
        .await
        .unwrap();
    let approval = fixture
        .service
        .request_activation(
            &fixture.actor([SkillGrant::Activate]),
            &candidate.revision_id,
        )
        .await
        .unwrap();
    let active_record = fixture
        .state
        .get_revision(&active.revision_id)
        .await
        .unwrap()
        .unwrap();
    let active_installation = fixture
        .state
        .get_installation(&active.package_id)
        .await
        .unwrap()
        .unwrap();
    let candidate_record = fixture
        .state
        .get_revision(&candidate.revision_id)
        .await
        .unwrap()
        .unwrap();
    let initial_snapshot_rows = snapshot_rows(&fixture).await;
    let initial_audit_count = audit_count(&fixture).await;
    let generation = fixture.manager.current_snapshot().generation();
    let prepare =
        faults.gate_once(crate::skill_store::SkillStoreFaultPoint::ActivationAfterPrepare);
    let built =
        faults.gate_once(crate::skill_store::SkillStoreFaultPoint::ActivationAfterCandidateBuild);
    let compensated =
        faults.gate_once(crate::skill_store::SkillStoreFaultPoint::ActivationAfterCompensation);
    let mut events = fixture.service.subscribe_events();
    let service = fixture.service.clone();
    let approval_id = approval.approval_id.clone();
    let approver = ActorContext::owner("approver-3", [SkillGrant::Activate]);
    let waiter =
        tokio::spawn(async move { service.approve_activation(&approval_id, &approver).await });
    tokio::time::timeout(std::time::Duration::from_secs(1), prepare.wait_entered())
        .await
        .expect("activation must reach prepare gate");
    corrupt_managed_instructions(Path::new(&active_record.storage_path)).await;

    waiter.abort();
    prepare.release().await;
    tokio::time::timeout(std::time::Duration::from_secs(1), built.wait_entered())
        .await
        .expect("activation must reach candidate-build gate");
    built.release().await;
    tokio::time::timeout(
        std::time::Duration::from_secs(1),
        compensated.wait_entered(),
    )
    .await
    .expect("activation build failure must reach compensation gate");

    assert_eq!(
        fixture
            .state
            .get_revision(&active.revision_id)
            .await
            .unwrap()
            .unwrap(),
        active_record
    );
    assert_eq!(
        fixture
            .state
            .get_installation(&active.package_id)
            .await
            .unwrap()
            .unwrap(),
        active_installation
    );
    assert_eq!(
        fixture
            .state
            .get_revision(&candidate.revision_id)
            .await
            .unwrap()
            .unwrap(),
        candidate_record
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
    assert!(Path::new(&active_record.storage_path).is_dir());
    assert!(Path::new(&candidate_record.storage_path).is_dir());
    assert_eq!(fixture.manager.current_snapshot().generation(), generation);
    assert_eq!(snapshot_rows(&fixture).await, initial_snapshot_rows);
    assert_eq!(audit_count(&fixture).await, initial_audit_count);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
    compensated.release().await;
}

async fn create_package(
    fixture: &AuthoringFixture,
    package_id: &str,
) -> crate::skill_management::SkillDraftSummary {
    fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse(package_id).unwrap(),
                display_name: "Review candidate".into(),
                description: "Review candidate instructions.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap()
}

async fn activate_package(
    fixture: &AuthoringFixture,
    package_id: &str,
) -> crate::skill_management::SkillDraftSummary {
    let draft = create_package(fixture, package_id).await;
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
    draft
}

async fn corrupt_managed_instructions(root: &Path) {
    let path = root.join("SKILL.md");
    let metadata = tokio::fs::metadata(&path).await.unwrap();
    let mut permissions = metadata.permissions();
    set_test_writable(&mut permissions);
    tokio::fs::set_permissions(&path, permissions)
        .await
        .unwrap();
    tokio::fs::write(path, "corrupt after activation")
        .await
        .unwrap();
}

fn set_test_writable(permissions: &mut std::fs::Permissions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o644);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
}

async fn snapshot_rows(fixture: &AuthoringFixture) -> Vec<(i64, String, String)> {
    sqlx::query_as(
        "SELECT generation, status, members_json FROM skill_snapshots ORDER BY generation",
    )
    .fetch_all(fixture.state.pool())
    .await
    .unwrap()
}

async fn audit_count(fixture: &AuthoringFixture) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM skill_audit_log")
        .fetch_one(fixture.state.pool())
        .await
        .unwrap()
}

async fn directory_entry_count(root: &Path) -> usize {
    let mut entries = tokio::fs::read_dir(root).await.unwrap();
    let mut count = 0;
    while entries.next_entry().await.unwrap().is_some() {
        count += 1;
    }
    count
}
