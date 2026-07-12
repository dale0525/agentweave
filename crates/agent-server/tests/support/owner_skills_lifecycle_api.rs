use super::*;
use agent_runtime::skill_management::CreateSkillDraftRequest;
use agent_runtime::skill_package::SkillPackageKind;

fn lifecycle_actor(id: &str) -> ActorContext {
    ActorContext::owner(
        id,
        [
            SkillGrant::Inspect,
            SkillGrant::CreateDraft,
            SkillGrant::Validate,
            SkillGrant::Activate,
            SkillGrant::Rollback,
            SkillGrant::Disable,
            SkillGrant::DeleteManaged,
        ],
    )
}

async fn activate(
    service: &OwnerSkillManagementService,
    package_id: &str,
    requester: &ActorContext,
    approver: &ActorContext,
) -> String {
    let draft = service
        .create_draft(
            requester,
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse(package_id).unwrap(),
                display_name: package_id.into(),
                description: "Lifecycle API package.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    service
        .validate_draft(requester, &draft.revision_id)
        .await
        .unwrap();
    let approval = service
        .request_activation(requester, &draft.revision_id)
        .await
        .unwrap();
    service
        .approve_activation(&approval.approval_id, approver)
        .await
        .unwrap();
    draft.revision_id
}

#[tokio::test]
async fn authorized_owner_routes_roll_back_and_disable_managed_skill() {
    let owner = lifecycle_actor("owner-1");
    let approver = lifecycle_actor("approver-2");
    let auth = OwnerAuth::from_principals([
        ("owner-token", owner.clone()),
        ("approver-token", approver.clone()),
    ])
    .unwrap();
    let test = owner_test_app_with_auth(
        SkillManagementPolicy::owner_only(),
        auth,
        SkillStoreLimits::default(),
    )
    .await;
    let first = activate(&test.service, "com.example.lifecycle", &owner, &approver).await;
    activate(&test.service, "com.example.lifecycle", &owner, &approver).await;

    let rollback = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/com.example.lifecycle/rollback",
            Some("Bearer owner-token"),
            Some(json!({"revision_id": first})),
        ))
        .await
        .unwrap();
    assert_eq!(rollback.status(), StatusCode::OK);
    let rollback = response_json(rollback).await;
    assert_eq!(rollback["active_revision_id"], first);

    let disable = test
        .app
        .oneshot(request(
            "POST",
            "/owner/skills/com.example.lifecycle/disable",
            Some("Bearer owner-token"),
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(disable.status(), StatusCode::OK);
    assert!(test.manager.current_snapshot().packages().is_empty());
}

#[tokio::test]
async fn removal_route_uses_bound_different_actor_approval() {
    let owner = lifecycle_actor("owner-1");
    let approver = lifecycle_actor("approver-2");
    let auth = OwnerAuth::from_principals([
        ("owner-token", owner.clone()),
        ("approver-token", approver.clone()),
    ])
    .unwrap();
    let test = owner_test_app_with_auth(
        SkillManagementPolicy::owner_only(),
        auth,
        SkillStoreLimits::default(),
    )
    .await;
    activate(&test.service, "com.example.remove", &owner, &approver).await;

    let requested = test
        .app
        .clone()
        .oneshot(request(
            "DELETE",
            "/owner/skills/com.example.remove",
            Some("Bearer owner-token"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(requested.status(), StatusCode::ACCEPTED);
    let approval_id = response_json(requested).await["approval_id"]
        .as_str()
        .unwrap()
        .to_string();

    let own = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/owner/skills/approvals/{approval_id}"),
            Some("Bearer owner-token"),
            Some(json!({"decision": "approve"})),
        ))
        .await
        .unwrap();
    assert_eq!(own.status(), StatusCode::CONFLICT);

    let approved = test
        .app
        .oneshot(request(
            "POST",
            &format!("/owner/skills/approvals/{approval_id}"),
            Some("Bearer approver-token"),
            Some(json!({"decision": "approve"})),
        ))
        .await
        .unwrap();
    assert_eq!(approved.status(), StatusCode::OK);
    assert!(test.manager.current_snapshot().packages().is_empty());
}
