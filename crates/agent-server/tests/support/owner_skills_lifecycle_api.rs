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

#[tokio::test]
async fn rollback_policy_returns_accepted_then_resolves_through_bound_approval_route() {
    let owner = lifecycle_actor("owner-1");
    let approver = lifecycle_actor("approver-2");
    let auth = OwnerAuth::from_principals([
        ("owner-token", owner.clone()),
        ("approver-token", approver.clone()),
    ])
    .unwrap();
    let mut policy = SkillManagementPolicy::owner_only();
    policy.rollback_approval_required = true;
    let test = owner_test_app_with_auth(policy, auth, SkillStoreLimits::default()).await;
    let first = activate(
        &test.service,
        "com.example.rollback-approval",
        &owner,
        &approver,
    )
    .await;
    activate(
        &test.service,
        "com.example.rollback-approval",
        &owner,
        &approver,
    )
    .await;

    let requested = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/com.example.rollback-approval/rollback",
            Some("Bearer owner-token"),
            Some(json!({"revision_id": first})),
        ))
        .await
        .unwrap();
    assert_eq!(requested.status(), StatusCode::ACCEPTED);
    let approval_id = response_json(requested).await["approval_id"]
        .as_str()
        .unwrap()
        .to_string();

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
    assert_eq!(
        test.state
            .get_installation(&SkillPackageId::parse("com.example.rollback-approval").unwrap(),)
            .await
            .unwrap()
            .unwrap()
            .active_revision_id
            .as_deref(),
        Some(first.as_str())
    );
}

#[tokio::test]
async fn lifecycle_auth_precedes_malformed_and_oversized_request_bodies() {
    let auth = OwnerAuth::new("owner-token", lifecycle_actor("owner-1")).unwrap();
    let test = owner_test_app_with_auth(
        SkillManagementPolicy::owner_only(),
        auth,
        SkillStoreLimits::default(),
    )
    .await;
    let malformed = "{".to_string();
    let oversized = format!("{{\"padding\":\"{}\"}}", "x".repeat(2 * 1024 * 1024));

    for uri in [
        "/owner/skills/com.example.lifecycle/rollback",
        "/owner/skills/com.example.lifecycle/disable",
    ] {
        for body in [&malformed, &oversized] {
            let response = test
                .app
                .clone()
                .oneshot(lifecycle_raw_request(uri, body.clone()))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
            assert_eq!(
                response_json(response).await,
                json!({"error": "authentication required"})
            );
        }
    }
}

fn lifecycle_raw_request(uri: &str, body: String) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}
