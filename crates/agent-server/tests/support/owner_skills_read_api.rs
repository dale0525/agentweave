use super::*;

#[tokio::test]
async fn authenticated_principal_returns_authoritative_actor_grants_and_policy() {
    let actor = ActorContext::owner("limited-owner", [SkillGrant::Inspect, SkillGrant::Validate]);
    let test = owner_test_app(SkillManagementPolicy::owner_only(), "limited-token", actor).await;
    let response = test
        .app
        .oneshot(request(
            "GET",
            "/owner/principal",
            Some("Bearer limited-token"),
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response_json(response).await,
        json!({
            "actorId": "limited-owner",
            "role": "owner",
            "grants": ["inspect", "validate"],
            "policy": {
                "mode": "owner_only",
                "agent_authoring": true,
                "allowed_kinds": ["instruction_only", "host_tools_only"],
                "protected_packages": [],
                "allowed_overrides": [],
                "activation_approval_required": true,
                "permission_escalation_approval_required": true,
                "rollback_approval_required": false
            }
        })
    );
}

#[tokio::test]
async fn package_detail_uses_persisted_revision_and_editable_draft_content() {
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        task10_actor("owner-1"),
    )
    .await;
    let created = test
        .service
        .create_draft(
            &task10_actor("owner-1"),
            serde_json::from_value(draft_body("com.example.detail")).unwrap(),
        )
        .await
        .unwrap();
    let response = test
        .app
        .oneshot(request(
            "GET",
            "/owner/skills/com.example.detail/detail",
            Some("Bearer secret-token"),
            None,
        ))
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
    let body = response_json(response).await;
    assert_eq!(body["package_id"], "com.example.detail");
    assert_eq!(body["display_name"], "Calendar");
    assert_eq!(body["active_revision_id"], Value::Null);
    assert_eq!(body["editable_draft"]["revision_id"], created.revision_id);
    assert_eq!(body["editable_draft"]["status"], "staging");
    assert_eq!(body["editable_draft"]["editable"], true);
    assert_eq!(
        body["editable_draft"]["instructions"],
        "---\nname: com-example-detail\ndescription: \"Calendar workflow.\"\n---\n\n# Calendar\n\nCalendar workflow.\n"
    );
    assert_eq!(body["revisions"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["revisions"][0]["requirements"],
        json!({
            "runtime_tools": [],
            "capabilities": [],
            "connectors": [],
            "packages": []
        })
    );
}
