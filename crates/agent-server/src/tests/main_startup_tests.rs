use super::*;

#[tokio::test]
async fn production_owner_initialization_awaits_startup_reconciliation() {
    let root = unique_test_dir("startup-reconcile-skills");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let app_root = unique_test_dir("startup-reconcile-app");
    let cache_root = unique_test_dir("startup-reconcile-cache");
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let loaded = load_skill_manager(
        &root,
        storage.clone(),
        Some(ManagedSkillsConfig {
            app_data_root: app_root.clone(),
            cache_root: cache_root.clone(),
        }),
    )
    .await
    .unwrap();
    let store = loaded.managed_store.clone().unwrap();
    let source = unique_test_dir("startup-reconcile-source");
    write_instruction_package(&source, "com.example.startup-row").await;
    let draft = store
        .create_staging_revision(&source, "owner")
        .await
        .unwrap();
    tokio::fs::remove_dir_all(&draft.path).await.unwrap();

    let config = build_owner_api_config(
        Some(test_owner_host()),
        &loaded,
        storage.clone(),
        Vec::new(),
    )
    .await
    .unwrap();

    assert!(config.is_some());
    assert!(
        SkillStateStore::new(storage)
            .get_revision(&draft.revision_id)
            .await
            .unwrap()
            .is_none()
    );
    remove_test_dir(root).await;
    remove_test_dir(app_root).await;
    remove_test_dir(cache_root).await;
    remove_test_dir(source).await;
}

#[tokio::test]
async fn production_owner_initialization_fails_closed_when_reconciliation_fails() {
    let root = unique_test_dir("startup-failure-skills");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let app_root = unique_test_dir("startup-failure-app");
    let cache_root = unique_test_dir("startup-failure-cache");
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let loaded = load_skill_manager(
        &root,
        storage.clone(),
        Some(ManagedSkillsConfig {
            app_data_root: app_root.clone(),
            cache_root: cache_root.clone(),
        }),
    )
    .await
    .unwrap();
    tokio::fs::remove_dir_all(&app_root).await.unwrap();

    let result =
        build_owner_api_config(Some(test_owner_host()), &loaded, storage, Vec::new()).await;

    assert!(result.is_err());
    remove_test_dir(root).await;
    remove_test_dir(app_root).await;
    remove_test_dir(cache_root).await;
}

fn test_owner_host() -> OwnerHostConfig {
    owner_host_config_from_lookup(|name| match name {
        "GENERAL_AGENT_SKILL_MANAGEMENT_MODE" => Some("owner_only".into()),
        "GENERAL_AGENT_OWNER_TOKEN" => Some("owner-token".into()),
        "GENERAL_AGENT_APPROVER_TOKEN" => Some("approver-token".into()),
        _ => None,
    })
    .unwrap()
    .unwrap()
}

#[tokio::test]
async fn environment_owner_mounts_all_task11_lifecycle_routes() {
    let root = unique_test_dir("owner-lifecycle-routes-skills");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let app_root = unique_test_dir("owner-lifecycle-routes-app");
    let cache_root = unique_test_dir("owner-lifecycle-routes-cache");
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let loaded = load_skill_manager(
        &root,
        storage.clone(),
        Some(ManagedSkillsConfig {
            app_data_root: app_root.clone(),
            cache_root: cache_root.clone(),
        }),
    )
    .await
    .unwrap();
    let owner = build_owner_api_config(
        Some(test_owner_host()),
        &loaded,
        storage.clone(),
        Vec::new(),
    )
    .await
    .unwrap()
    .unwrap();
    let state = Arc::new(api::AppState::new_with_model_skill_manager_and_owner(
        storage,
        CapturingModel {
            tool_names: Arc::new(Mutex::new(Vec::new())),
        },
        loaded.manager,
        RuntimeConfig::read_only(".", ".").without_builtin_tools(),
        owner,
    ));
    let app = api::router(state);
    for uri in [
        "/owner/skills/com.example.missing/rollback",
        "/owner/skills/com.example.missing/disable",
        "/owner/skills/com.example.missing",
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(uri)
                    .header("authorization", "Bearer owner-token")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{uri}");
    }
    remove_test_dir(root).await;
    remove_test_dir(app_root).await;
    remove_test_dir(cache_root).await;
}

#[tokio::test]
async fn production_loader_does_not_mutate_corrupt_active_before_lkg_reconcile() {
    let root = unique_test_dir("lkg-first-skills");
    tokio::fs::create_dir_all(&root).await.unwrap();
    let app_root = unique_test_dir("lkg-first-app");
    let cache_root = unique_test_dir("lkg-first-cache");
    let storage = Storage::connect("sqlite::memory:").await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let paths = SkillStorePaths::prepare(&app_root, &cache_root)
        .await
        .unwrap();
    let store = SkillRevisionStore::new(paths, state.clone());
    let source_one = unique_test_dir("lkg-first-source-one");
    let source_two = unique_test_dir("lkg-first-source-two");
    write_instruction_package(&source_one, "com.example.lkg-first").await;
    write_instruction_package(&source_two, "com.example.lkg-first").await;
    let good = store
        .create_staging_revision(&source_one, "owner")
        .await
        .unwrap();
    let good = store.promote_revision(&good.revision_id).await.unwrap();
    state
        .activate_revision(
            &SkillPackageId::parse("com.example.lkg-first").unwrap(),
            &good.revision_id,
            SkillLayerRecord::Managed,
            "owner",
        )
        .await
        .unwrap();
    let bad = store
        .create_staging_revision(&source_two, "owner")
        .await
        .unwrap();
    let bad = store.promote_revision(&bad.revision_id).await.unwrap();
    state
        .activate_revision(
            &SkillPackageId::parse("com.example.lkg-first").unwrap(),
            &bad.revision_id,
            SkillLayerRecord::Managed,
            "owner",
        )
        .await
        .unwrap();
    let good_record = state
        .get_revision(&good.revision_id)
        .await
        .unwrap()
        .unwrap();
    let bad_record = state.get_revision(&bad.revision_id).await.unwrap().unwrap();
    let member = |revision: &agent_runtime::skill_state::SkillRevisionRecord| {
        serde_json::json!([{
            "packageId": revision.package_id.as_str(),
            "version": revision.version,
            "contentHash": revision.content_hash,
            "layer": "managed",
            "revisionId": revision.revision_id,
        }])
    };
    state
        .record_snapshot_candidate(1, member(&good_record))
        .await
        .unwrap();
    state.record_snapshot_activation(1).await.unwrap();
    state.mark_snapshot_last_known_good(1).await.unwrap();
    state
        .record_snapshot_candidate(2, member(&bad_record))
        .await
        .unwrap();
    state.record_snapshot_activation(2).await.unwrap();
    make_test_tree_writable(&bad.path).await;
    tokio::fs::write(bad.path.join("general-agent.json"), b"{}")
        .await
        .unwrap();
    let package_id = SkillPackageId::parse("com.example.lkg-first").unwrap();
    let audits_before = state.list_audit(&package_id).await.unwrap().len();

    let loaded = load_skill_manager(
        &root,
        storage.clone(),
        Some(ManagedSkillsConfig {
            app_data_root: app_root.clone(),
            cache_root: cache_root.clone(),
        }),
    )
    .await
    .unwrap();

    assert_eq!(
        state
            .get_revision(&bad.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        agent_runtime::skill_state::SkillRevisionStatus::Managed
    );
    assert!(bad.path.is_dir());
    assert!(!store.paths().quarantine.join(&bad.revision_id).exists());
    let audits_after_load = state.list_audit(&package_id).await.unwrap().len();
    assert_eq!(audits_after_load, audits_before);
    assert_eq!(
        state.get_snapshot(2).await.unwrap().unwrap().status,
        agent_runtime::skill_state::SkillSnapshotStatus::Active
    );

    build_owner_api_config(Some(test_owner_host()), &loaded, storage, Vec::new())
        .await
        .unwrap();

    assert_eq!(
        state
            .get_revision(&bad.revision_id)
            .await
            .unwrap()
            .unwrap()
            .status,
        agent_runtime::skill_state::SkillRevisionStatus::Quarantined
    );
    assert_eq!(loaded.manager.current_snapshot().generation(), 1);
    remove_test_dir(root).await;
    remove_test_dir(app_root).await;
    remove_test_dir(cache_root).await;
    remove_test_dir(source_one).await;
    remove_test_dir(source_two).await;
}
