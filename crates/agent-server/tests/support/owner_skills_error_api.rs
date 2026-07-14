use super::*;

#[tokio::test]
async fn task10_service_errors_map_to_stable_http_boundaries_without_leaks() {
    let token = Some("Bearer secret-token");
    let test = owner_test_app(
        SkillManagementPolicy::owner_only(),
        "secret-token",
        task10_actor("owner-1"),
    )
    .await;

    let malformed = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/approvals/not-a-uuid",
            token,
            Some(json!({"decision": "approve"})),
        ))
        .await
        .unwrap();
    assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);

    let malformed_revision = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts/private-not-a-revision/validate",
            token,
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(malformed_revision.status(), StatusCode::BAD_REQUEST);
    let malformed_body = response_json(malformed_revision).await.to_string();
    assert!(!malformed_body.contains("private-not-a-revision"));
    assert!(!malformed_body.contains("skill_revisions"));
    assert!(!malformed_body.contains("secret-token"));
    assert!(!malformed_body.contains(test.roots.app_root.to_string_lossy().as_ref()));

    let missing = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts/00000000-0000-4000-8000-000000000001/validate",
            token,
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let missing_export = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/com.example.missing/export",
            token,
            Some(json!({"name": "missing"})),
        ))
        .await
        .unwrap();
    assert_eq!(missing_export.status(), StatusCode::NOT_FOUND);

    let created = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts",
            token,
            Some(draft_body("com.example.error-boundary")),
        ))
        .await
        .unwrap();
    assert_eq!(created.status(), StatusCode::CREATED);
    let revision_id = response_json(created).await["revision_id"]
        .as_str()
        .unwrap()
        .to_string();
    let invalid_update = test
        .app
        .clone()
        .oneshot(request(
            "PUT",
            &format!("/owner/skills/drafts/{revision_id}"),
            token,
            Some(json!({"files": [{
                "path": "agentweave.json",
                "content": "{ private malformed descriptor"
            }]})),
        ))
        .await
        .unwrap();
    assert_eq!(invalid_update.status(), StatusCode::BAD_REQUEST);
    let invalid_body = response_json(invalid_update).await.to_string();
    assert!(!invalid_body.contains("private malformed descriptor"));
    assert!(!invalid_body.contains(test.roots.app_root.to_string_lossy().as_ref()));

    let malformed_import_root = test.roots.import_root.join("malformed");
    std::fs::create_dir_all(&malformed_import_root).unwrap();
    std::fs::write(malformed_import_root.join("agentweave.json"), "{ bad").unwrap();
    std::fs::write(malformed_import_root.join("SKILL.md"), "# Bad").unwrap();
    let invalid_import = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            "/owner/skills/drafts/import",
            token,
            Some(json!({"name": "malformed"})),
        ))
        .await
        .unwrap();
    assert_eq!(invalid_import.status(), StatusCode::BAD_REQUEST);

    test.store.promote_revision(&revision_id).await.unwrap();
    let wrong_lifecycle = test
        .app
        .clone()
        .oneshot(request(
            "POST",
            &format!("/owner/skills/drafts/{revision_id}/test"),
            token,
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(wrong_lifecycle.status(), StatusCode::CONFLICT);

    let internal_draft = test
        .service
        .create_draft(
            &task10_actor("owner-1"),
            serde_json::from_value(draft_body("com.example.internal-boundary")).unwrap(),
        )
        .await
        .unwrap();
    let staging = test.store.paths().staging.clone();
    std::fs::rename(&staging, staging.with_extension("moved")).unwrap();
    std::fs::create_dir(&staging).unwrap();
    let internal = test
        .app
        .oneshot(request(
            "POST",
            &format!(
                "/owner/skills/drafts/{}/validate",
                internal_draft.revision_id
            ),
            token,
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert_eq!(internal.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = response_json(internal).await.to_string();
    assert!(!body.contains("skill_revisions"));
    assert!(!body.contains("secret-token"));
    assert!(!body.contains(test.roots.app_root.to_string_lossy().as_ref()));
}
