use crate::events::RuntimeEvent;
use crate::skill_authoring_tests::{AuthoringFixture, update};
use crate::skill_policy::{ActorContext, SkillGrant};
use crate::skill_state::SkillApprovalStatus;

async fn validate_for_activation(
    fixture: &AuthoringFixture,
) -> crate::skill_management::SkillDraftSummary {
    let draft = fixture.draft().await;
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    draft
}

#[tokio::test]
async fn activation_request_requires_validation_and_deduplicates_exact_candidate() {
    let fixture = AuthoringFixture::new().await;
    let mut events = fixture.service.subscribe_events();
    let draft = fixture.draft().await;
    let requester = fixture.actor([SkillGrant::Activate]);
    assert!(
        fixture
            .service
            .request_activation(&requester, &draft.revision_id)
            .await
            .unwrap_err()
            .to_string()
            .contains("validation")
    );
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();

    let (first, second) = tokio::join!(
        fixture
            .service
            .request_activation(&requester, &draft.revision_id),
        fixture
            .service
            .request_activation(&requester, &draft.revision_id),
    );
    let first = first.unwrap();
    let second = second.unwrap();

    assert_eq!(first.approval_id, second.approval_id);
    assert_eq!(first.status, SkillApprovalStatus::Pending);
    assert_eq!(first.requested_by, "owner-1");
    assert!(first.permission_diff.get("binding").is_none());
    assert!(matches!(
        events.recv().await.unwrap(),
        RuntimeEvent::SkillApprovalRequired { .. }
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn activation_binding_is_private_and_events_are_broadcast_once() {
    let fixture = AuthoringFixture::new().await;
    let draft = validate_for_activation(&fixture).await;
    let validation = fixture
        .state
        .revision_validation(&draft.revision_id)
        .await
        .unwrap();
    let mut events = fixture.service.subscribe_events();
    let requester = fixture.actor([SkillGrant::Activate]);

    let first = fixture
        .service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();
    let second = fixture
        .service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();

    assert_eq!(first.approval_id, second.approval_id);
    assert_eq!(first.permission_diff, validation["permissionDiff"]);
    assert!(first.permission_diff.get("binding").is_none());
    assert!(matches!(
        events.recv().await.unwrap(),
        RuntimeEvent::SkillApprovalRequired { approval_id, .. }
            if approval_id == first.approval_id
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );

    let binding: String = sqlx::query_scalar(
        "SELECT binding_json FROM skill_approval_bindings WHERE approval_id = ?",
    )
    .bind(&first.approval_id)
    .fetch_one(fixture.state.pool())
    .await
    .unwrap();
    let binding: serde_json::Value = serde_json::from_str(&binding).unwrap();
    assert_eq!(binding["revisionId"], draft.revision_id);
    assert_eq!(binding["contentHash"], validation["contentHash"]);
    assert_eq!(binding["validationSnapshotGeneration"], 1);
    assert_eq!(binding["requestingActor"], "owner-1");
}

#[tokio::test]
async fn approval_requires_different_actor_is_single_use_and_publishes_once() {
    let fixture = AuthoringFixture::new().await;
    let mut events = fixture.service.subscribe_events();
    let draft = validate_for_activation(&fixture).await;
    let requester = fixture.actor([SkillGrant::Activate]);
    let approval = fixture
        .service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();
    let self_error = fixture
        .service
        .approve_activation(&approval.approval_id, &requester)
        .await
        .unwrap_err();
    assert!(matches!(
        self_error.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
    let approver = ActorContext::owner("approver-2", [SkillGrant::Activate]);

    let report = fixture
        .service
        .approve_activation(&approval.approval_id, &approver)
        .await
        .unwrap();

    assert_eq!(report.previous_generation, 1);
    assert_eq!(report.active_generation, 2);
    assert!(
        fixture
            .manager
            .current_snapshot()
            .packages()
            .iter()
            .any(|item| { item.package.descriptor.id == draft.package_id })
    );
    let duplicate = fixture
        .service
        .approve_activation(&approval.approval_id, &approver)
        .await
        .unwrap_err();
    assert!(matches!(
        duplicate.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
    assert!(matches!(
        events.recv().await.unwrap(),
        RuntimeEvent::SkillApprovalRequired { .. }
    ));
    assert!(matches!(
        events.recv().await.unwrap(),
        RuntimeEvent::SkillSnapshotPublished { generation: 2 }
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn edit_makes_old_approval_stale_and_new_request_gets_new_binding() {
    let fixture = AuthoringFixture::new().await;
    let draft = validate_for_activation(&fixture).await;
    let requester = fixture.actor([SkillGrant::Activate]);
    let old = fixture
        .service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();
    fixture
        .service
        .update_draft(
            &fixture.actor([SkillGrant::EditDraft]),
            &draft.revision_id,
            vec![update("references/change.md", "changed\n")],
        )
        .await
        .unwrap();
    let approver = ActorContext::owner("approver-2", [SkillGrant::Activate]);
    let stale = fixture
        .service
        .approve_activation(&old.approval_id, &approver)
        .await
        .unwrap_err();
    assert!(matches!(
        stale.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Conflict { .. })
    ));
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &draft.revision_id)
        .await
        .unwrap();
    let new = fixture
        .service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();
    assert_ne!(new.approval_id, old.approval_id);
}

#[tokio::test]
async fn concurrent_approval_publishes_one_generation() {
    let fixture = AuthoringFixture::new().await;
    let draft = validate_for_activation(&fixture).await;
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    let approver = ActorContext::owner("approver-2", [SkillGrant::Activate]);
    let (left, right) = tokio::join!(
        fixture
            .service
            .approve_activation(&approval.approval_id, &approver),
        fixture
            .service
            .approve_activation(&approval.approval_id, &approver),
    );

    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(fixture.manager.current_snapshot().generation(), 2);
}

#[tokio::test]
async fn different_package_approvals_publish_serial_exact_snapshots() {
    let fixture = AuthoringFixture::new().await;
    let first = validate_for_activation(&fixture).await;
    let second = fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            crate::skill_management::CreateSkillDraftRequest {
                package_id: crate::skill_package::SkillPackageId::parse("com.example.tasks")
                    .unwrap(),
                display_name: "Tasks".into(),
                description: "Guide task planning.".into(),
                kind: crate::skill_package::SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &second.revision_id)
        .await
        .unwrap();
    let requester = fixture.actor([SkillGrant::Activate]);
    let first_approval = fixture
        .service
        .request_activation(&requester, &first.revision_id)
        .await
        .unwrap();
    let second_approval = fixture
        .service
        .request_activation(&requester, &second.revision_id)
        .await
        .unwrap();
    let approver = ActorContext::owner("approver-2", [SkillGrant::Activate]);

    let (left, right) = tokio::join!(
        fixture
            .service
            .approve_activation(&first_approval.approval_id, &approver),
        fixture
            .service
            .approve_activation(&second_approval.approval_id, &approver),
    );

    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    let generation_two = fixture.state.get_snapshot(2).await.unwrap().unwrap();
    assert_eq!(generation_two.members_json.as_array().unwrap().len(), 1);
    let (loser, stale_approval) = if left.is_ok() {
        (&second, &second_approval)
    } else {
        (&first, &first_approval)
    };
    assert_eq!(
        fixture
            .state
            .get_revision(&loser.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        crate::skill_state::SkillRevisionStatus::Staging
    );
    assert_eq!(
        fixture
            .state
            .get_approval(&stale_approval.approval_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        SkillApprovalStatus::Pending
    );
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &loser.revision_id)
        .await
        .unwrap();
    let refreshed = fixture
        .service
        .request_activation(&requester, &loser.revision_id)
        .await
        .unwrap();
    fixture
        .service
        .approve_activation(&refreshed.approval_id, &approver)
        .await
        .unwrap();
    let generation_three = fixture.state.get_snapshot(3).await.unwrap().unwrap();
    assert_eq!(generation_three.members_json.as_array().unwrap().len(), 2);
    assert_eq!(fixture.manager.current_snapshot().generation(), 3);
}

#[tokio::test]
async fn same_package_competing_revisions_publish_exactly_one() {
    let fixture = AuthoringFixture::new().await;
    let first = validate_for_activation(&fixture).await;
    let second = fixture
        .service
        .create_draft(
            &fixture.actor([SkillGrant::CreateDraft]),
            crate::skill_management::CreateSkillDraftRequest {
                package_id: first.package_id.clone(),
                display_name: "Calendar second".into(),
                description: "A competing calendar revision.".into(),
                kind: crate::skill_package::SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    fixture
        .service
        .validate_draft(&fixture.actor([SkillGrant::Validate]), &second.revision_id)
        .await
        .unwrap();
    let requester = fixture.actor([SkillGrant::Activate]);
    let first_approval = fixture
        .service
        .request_activation(&requester, &first.revision_id)
        .await
        .unwrap();
    let second_approval = fixture
        .service
        .request_activation(&requester, &second.revision_id)
        .await
        .unwrap();
    let approver = ActorContext::owner("approver-2", [SkillGrant::Activate]);

    let (left, right) = tokio::join!(
        fixture
            .service
            .approve_activation(&first_approval.approval_id, &approver),
        fixture
            .service
            .approve_activation(&second_approval.approval_id, &approver),
    );

    assert_eq!(usize::from(left.is_ok()) + usize::from(right.is_ok()), 1);
    assert_eq!(fixture.manager.current_snapshot().generation(), 2);
    let installation = fixture
        .state
        .get_installation(&first.package_id)
        .await
        .unwrap()
        .unwrap();
    let winner = installation.active_revision_id.unwrap();
    assert!(winner == first.revision_id || winner == second.revision_id);
}

#[tokio::test]
async fn reload_failure_keeps_old_snapshot_and_installation() {
    let fixture = AuthoringFixture::new().await;
    let mut events = fixture.service.subscribe_events();
    let draft = validate_for_activation(&fixture).await;
    let approval = fixture
        .service
        .request_activation(&fixture.actor([SkillGrant::Activate]), &draft.revision_id)
        .await
        .unwrap();
    sqlx::query("DROP TABLE skill_snapshots")
        .execute(fixture.state.pool())
        .await
        .unwrap();
    let error = fixture
        .service
        .approve_activation(
            &approval.approval_id,
            &ActorContext::owner("approver-2", [SkillGrant::Activate]),
        )
        .await
        .unwrap_err();

    assert!(matches!(
        error.downcast_ref::<crate::skill_management::SkillManagementError>(),
        Some(crate::skill_management::SkillManagementError::Internal { .. })
    ));
    assert_eq!(fixture.manager.current_snapshot().generation(), 1);
    assert!(
        fixture
            .state
            .get_installation(&draft.package_id)
            .await
            .unwrap()
            .is_none()
    );
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
        crate::skill_state::SkillRevisionStatus::Staging
    );
    assert!(matches!(
        events.recv().await.unwrap(),
        RuntimeEvent::SkillApprovalRequired { .. }
    ));
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), events.recv())
            .await
            .is_err()
    );
}
