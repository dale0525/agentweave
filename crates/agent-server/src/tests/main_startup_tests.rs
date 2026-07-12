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
