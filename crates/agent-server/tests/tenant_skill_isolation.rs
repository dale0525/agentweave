use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill_management::CreateSkillDraftRequest;
use agent_runtime::skill_package::{SkillPackageId, SkillPackageKind};
use agent_runtime::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use agent_runtime::skill_source::SkillSource;
use agent_server::tenant_skills::{
    FilesystemTenantSkillManagerFactory, TenantSkillManagerConfig, TenantSkillManagerRegistry,
    validate_tenant_id,
};
use std::sync::Arc;

#[tokio::test]
async fn tenant_roots_rows_revisions_and_snapshots_are_isolated() {
    let root = tempfile::tempdir().unwrap();
    let factory = FilesystemTenantSkillManagerFactory::new(TenantSkillManagerConfig {
        data_root: root.path().join("data"),
        cache_root: root.path().join("cache"),
        sources: Vec::<Arc<dyn SkillSource>>::new(),
        platform: PlatformId::Server,
        capabilities: CapabilitySet::server_runtime(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
        management_policy: SkillManagementPolicy::owner_only(),
    })
    .await
    .unwrap();
    let registry = TenantSkillManagerRegistry::new(factory);
    let alpha = registry.for_tenant("alpha").await.unwrap();
    let beta = registry.for_tenant("beta").await.unwrap();
    alpha.manager.startup_reconcile().await.unwrap();
    beta.manager.startup_reconcile().await.unwrap();

    let revision = activate_fixture(&alpha.management, "com.example.alpha").await;

    assert_ne!(alpha.data_root, beta.data_root);
    assert_ne!(alpha.database_path, beta.database_path);
    let canonical = root.path().canonicalize().unwrap();
    assert!(alpha.data_root.starts_with(canonical.join("data/tenants")));
    assert!(beta.data_root.starts_with(canonical.join("data/tenants")));
    assert!(alpha.state.get_revision(&revision).await.unwrap().is_some());
    assert!(beta.state.get_revision(&revision).await.unwrap().is_none());
    assert!(
        beta.state
            .get_installation(&package_id("com.example.alpha"))
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        beta.management
            .list_managed_skills(&owner("beta-owner"))
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(alpha.manager.current_snapshot().generation(), 2);
    assert_eq!(beta.manager.current_snapshot().generation(), 1);
}

#[tokio::test]
async fn registry_creates_one_manager_per_canonical_tenant_under_concurrency() {
    let root = tempfile::tempdir().unwrap();
    let factory = FilesystemTenantSkillManagerFactory::new(TenantSkillManagerConfig {
        data_root: root.path().join("data"),
        cache_root: root.path().join("cache"),
        sources: Vec::new(),
        platform: PlatformId::Server,
        capabilities: CapabilitySet::server_runtime(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
        management_policy: SkillManagementPolicy::owner_only(),
    })
    .await
    .unwrap();
    let registry = Arc::new(TenantSkillManagerRegistry::new(factory));
    let mut tasks = Vec::new();
    for _ in 0..24 {
        let registry = registry.clone();
        tasks.push(tokio::spawn(async move {
            registry.for_tenant("singleflight").await.unwrap()
        }));
    }
    let first = tasks.remove(0).await.unwrap();
    for task in tasks {
        let runtime = task.await.unwrap();
        assert!(Arc::ptr_eq(&first, &runtime));
    }
    assert_eq!(registry.manager_count(), 1);
}

#[test]
fn tenant_ids_reject_aliases_case_encoding_unicode_and_traversal() {
    assert_eq!(validate_tenant_id("alpha-1").unwrap(), "alpha-1");
    for value in [
        "",
        "Alpha",
        "alpha/../beta",
        "alpha\\beta",
        ".",
        "..",
        "%61lpha",
        "alpha%2fbeta",
        "alpha.",
        "álpha",
        "alpha_1",
    ] {
        assert!(validate_tenant_id(value).is_err(), "accepted {value:?}");
    }
}

#[cfg(unix)]
#[tokio::test]
async fn tenant_factory_rejects_symlinked_tenant_root_without_touching_target() {
    use std::os::unix::fs::symlink;

    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(root.path().join("data/tenants"))
        .await
        .unwrap();
    symlink(outside.path(), root.path().join("data/tenants/linked")).unwrap();
    let factory = FilesystemTenantSkillManagerFactory::new(TenantSkillManagerConfig {
        data_root: root.path().join("data"),
        cache_root: root.path().join("cache"),
        sources: Vec::new(),
        platform: PlatformId::Server,
        capabilities: CapabilitySet::server_runtime(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
        management_policy: SkillManagementPolicy::owner_only(),
    })
    .await
    .unwrap();
    let registry = TenantSkillManagerRegistry::new(factory);

    let error = registry.for_tenant("linked").await.unwrap_err();

    assert!(error.to_string().contains("real directory"));
    assert!(!outside.path().join("state.db").exists());
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}

async fn activate_fixture(
    service: &agent_runtime::skill_management::OwnerSkillManagementService,
    id: &str,
) -> String {
    let requester = owner("alpha-owner");
    let draft = service
        .create_draft(
            &requester,
            CreateSkillDraftRequest {
                package_id: package_id(id),
                display_name: "Alpha fixture".into(),
                description: "Alpha-only fixture.".into(),
                kind: SkillPackageKind::InstructionOnly,
                required_tools: Vec::new(),
            },
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
        .approve_activation(&approval.approval_id, &owner("alpha-approver"))
        .await
        .unwrap();
    draft.revision_id
}

fn owner(id: &str) -> ActorContext {
    ActorContext::owner(
        id,
        [
            SkillGrant::Inspect,
            SkillGrant::CreateDraft,
            SkillGrant::Validate,
            SkillGrant::Activate,
        ],
    )
}

fn package_id(value: &str) -> SkillPackageId {
    SkillPackageId::parse(value).unwrap()
}
