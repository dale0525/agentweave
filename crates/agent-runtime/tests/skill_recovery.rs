use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill_management::{CreateSkillDraftRequest, OwnerSkillManagementService};
use agent_runtime::skill_manager::{SkillManager, SkillManagerConfig};
use agent_runtime::skill_package::{SkillPackageId, SkillPackageKind};
use agent_runtime::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use agent_runtime::skill_recovery::RecoveryStatus;
use agent_runtime::skill_source::ManagedSkillSource;
use agent_runtime::skill_state::SkillStateStore;
use agent_runtime::skill_store::{SkillRevisionStore, SkillStorePaths};
use agent_runtime::storage::Storage;
use serde_json::json;
use std::sync::Arc;
use tempfile::tempdir;

#[tokio::test]
async fn restart_restores_verified_lkg_after_active_bytes_are_corrupted() {
    let root = tempdir().unwrap();
    let app = root.path().join("app");
    let cache = root.path().join("cache");
    tokio::fs::create_dir_all(&app).await.unwrap();
    tokio::fs::create_dir_all(&cache).await.unwrap();
    let database = format!(
        "sqlite://{}?mode=rwc",
        root.path().join("state.db").display()
    );
    let storage = Storage::connect(&database).await.unwrap();
    let state = SkillStateStore::new(storage.clone());
    let paths = SkillStorePaths::prepare(&app, &cache).await.unwrap();
    let store = SkillRevisionStore::new(paths, state.clone());
    let manager = managed_manager(store.clone()).await;
    let service = OwnerSkillManagementService::new(
        manager.clone(),
        store.clone(),
        state.clone(),
        SkillManagementPolicy::owner_only(),
    );
    let first = activate(&service, &state, "1.0.0").await;
    let second = activate(&service, &state, "2.0.0").await;
    let second_record = state.get_revision(&second).await.unwrap().unwrap();
    let descriptor = std::path::Path::new(&second_record.storage_path).join("agentweave.json");
    make_file_writable(&descriptor).await;
    tokio::fs::write(&descriptor, b"corrupt").await.unwrap();
    drop(service);
    drop(manager);
    drop(store);
    drop(state);
    drop(storage);

    let restarted_storage = Storage::connect(&database).await.unwrap();
    let restarted_state = SkillStateStore::new(restarted_storage);
    let restarted_paths = SkillStorePaths::prepare(&app, &cache).await.unwrap();
    let restarted_store = SkillRevisionStore::new(restarted_paths, restarted_state.clone());
    let restarted_manager = managed_manager(restarted_store.clone()).await;
    let _service = OwnerSkillManagementService::new(
        restarted_manager.clone(),
        restarted_store,
        restarted_state.clone(),
        SkillManagementPolicy::owner_only(),
    );

    let report = restarted_manager.startup_reconcile().await.unwrap();

    assert_eq!(report.status, RecoveryStatus::LastKnownGoodRestored);
    let installation = restarted_state
        .get_installation(&SkillPackageId::parse("com.example.restart").unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        installation.active_revision_id.as_deref(),
        Some(first.as_str())
    );
    assert!(
        restarted_manager
            .current_snapshot()
            .packages()
            .iter()
            .any(|resolved| {
                resolved.package.descriptor.id.as_str() == "com.example.restart"
                    && resolved.package.descriptor.version.to_string() == "1.0.0"
            })
    );
}

async fn managed_manager(store: SkillRevisionStore) -> SkillManager {
    SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(ManagedSkillSource::from_store(store))],
        platform: PlatformId::Server,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
    })
    .await
    .unwrap()
}

async fn activate(
    service: &OwnerSkillManagementService,
    state: &SkillStateStore,
    version: &str,
) -> String {
    let requester = ActorContext::owner(
        "owner-1",
        [
            SkillGrant::CreateDraft,
            SkillGrant::EditDraft,
            SkillGrant::Validate,
            SkillGrant::Activate,
        ],
    );
    let draft = service
        .create_draft(
            &requester,
            CreateSkillDraftRequest {
                package_id: SkillPackageId::parse("com.example.restart").unwrap(),
                display_name: "Restart".into(),
                description: "Restart recovery package.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
        )
        .await
        .unwrap();
    let record = state
        .get_revision(&draft.revision_id)
        .await
        .unwrap()
        .unwrap();
    let mut descriptor = record.descriptor_json;
    descriptor["version"] = json!(version);
    service
        .update_draft(
            &requester,
            &draft.revision_id,
            vec![agent_runtime::skill_management::DraftFileUpdate {
                path: "agentweave.json".into(),
                content: format!("{}\n", serde_json::to_string_pretty(&descriptor).unwrap()),
            }],
        )
        .await
        .unwrap();
    service
        .validate_draft(&requester, &draft.revision_id)
        .await
        .unwrap();
    let approval = service
        .request_activation(&requester, &draft.revision_id)
        .await
        .unwrap();
    service
        .approve_activation(
            &approval.approval_id,
            &ActorContext::owner("approver-2", [SkillGrant::Activate]),
        )
        .await
        .unwrap();
    draft.revision_id
}

async fn make_file_writable(path: &std::path::Path) {
    let mut permissions = tokio::fs::metadata(path).await.unwrap().permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(0o600);
    }
    #[cfg(not(unix))]
    permissions.set_readonly(false);
    tokio::fs::set_permissions(path, permissions).await.unwrap();
}
