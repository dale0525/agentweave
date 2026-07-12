use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill_management::CreateSkillDraftRequest;
use agent_runtime::skill_package::{SkillPackageId, SkillPackageKind};
use agent_runtime::skill_policy::{ActorContext, SkillGrant, SkillManagementPolicy};
use agent_runtime::skill_source::{DiscoveredSkillPackage, SkillLayer, SkillSource};
use agent_server::tenant_skills::{
    FilesystemTenantSkillManagerFactory, TenantSkillManagerConfig, TenantSkillManagerFactory,
    TenantSkillManagerRegistry, TenantSkillRuntime, validate_tenant_id,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

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

#[tokio::test]
async fn concurrent_first_access_publishes_one_reconciled_active_generation() {
    let root = tempfile::tempdir().unwrap();
    let config = tenant_config(root.path(), Vec::new());
    let initial = TenantSkillManagerRegistry::new(
        FilesystemTenantSkillManagerFactory::new(config.clone())
            .await
            .unwrap(),
    );
    let runtime = initial.for_tenant("restarted").await.unwrap();
    let revision = activate_fixture(&runtime.management, "com.example.restarted").await;
    let expected_generation = runtime.manager.current_snapshot().generation();
    drop(runtime);
    drop(initial);

    let registry = Arc::new(TenantSkillManagerRegistry::new(
        FilesystemTenantSkillManagerFactory::new(config)
            .await
            .unwrap(),
    ));
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let registry = registry.clone();
        tasks.push(tokio::spawn(async move {
            registry.for_tenant("restarted").await.unwrap()
        }));
    }

    let first = tasks.remove(0).await.unwrap();
    assert_eq!(
        first.manager.current_snapshot().generation(),
        expected_generation
    );
    assert!(first.state.get_revision(&revision).await.unwrap().is_some());
    for task in tasks {
        let runtime = task.await.unwrap();
        assert!(Arc::ptr_eq(&first, &runtime));
        assert_eq!(
            runtime.manager.current_snapshot().generation(),
            expected_generation
        );
    }
}

#[tokio::test]
async fn concurrent_failed_initialization_removes_exact_entry_and_allows_clean_retry() {
    let root = tempfile::tempdir().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let factory = FailFirstFactory {
        delegate: FilesystemTenantSkillManagerFactory::new(tenant_config(root.path(), Vec::new()))
            .await
            .unwrap(),
        attempts: attempts.clone(),
    };
    let registry = Arc::new(TenantSkillManagerRegistry::new(factory));
    let mut tasks = Vec::new();
    for _ in 0..12 {
        let registry = registry.clone();
        tasks.push(tokio::spawn(async move {
            registry.for_tenant("retryable").await
        }));
    }

    for task in tasks {
        let error = task.await.unwrap().unwrap_err();
        assert!(error.to_string().contains("initialization failed"));
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(registry.entry_count(), 0);

    let runtime = registry.for_tenant("retryable").await.unwrap();
    assert_eq!(runtime.tenant_id, "retryable");
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(registry.entry_count(), 1);
}

#[tokio::test]
async fn cancelling_first_waiter_does_not_cancel_singleflight_initialization() {
    let root = tempfile::tempdir().unwrap();
    let attempts = Arc::new(AtomicUsize::new(0));
    let factory = DelayedFactory {
        delegate: FilesystemTenantSkillManagerFactory::new(tenant_config(root.path(), Vec::new()))
            .await
            .unwrap(),
        attempts: attempts.clone(),
    };
    let registry = Arc::new(TenantSkillManagerRegistry::new(factory));
    let first_registry = registry.clone();
    let first = tokio::spawn(async move { first_registry.for_tenant("cancelled").await });
    wait_for_attempt(&attempts).await;
    first.abort();
    assert!(first.await.unwrap_err().is_cancelled());

    let runtime = registry.for_tenant("cancelled").await.unwrap();

    assert_eq!(runtime.tenant_id, "cancelled");
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(registry.entry_count(), 1);
}

#[tokio::test]
async fn failed_filesystem_initialization_cleans_only_attempt_created_paths() {
    let root = tempfile::tempdir().unwrap();
    let factory = FilesystemTenantSkillManagerFactory::new(tenant_config(
        root.path(),
        vec![Arc::new(FailingSource)],
    ))
    .await
    .unwrap();
    let registry = TenantSkillManagerRegistry::new(factory);

    assert!(registry.for_tenant("fresh").await.is_err());

    assert!(!root.path().join("data/tenants/fresh").exists());
    assert!(!root.path().join("cache/tenants/fresh").exists());
    assert_eq!(registry.entry_count(), 0);
}

#[tokio::test]
async fn failed_filesystem_initialization_preserves_preexisting_tenant_data() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data/tenants/preserved");
    let cache = root.path().join("cache/tenants/preserved");
    tokio::fs::create_dir_all(&data).await.unwrap();
    tokio::fs::create_dir_all(&cache).await.unwrap();
    tokio::fs::write(data.join("keep.txt"), "data")
        .await
        .unwrap();
    tokio::fs::write(cache.join("keep.txt"), "cache")
        .await
        .unwrap();
    let factory = FilesystemTenantSkillManagerFactory::new(tenant_config(
        root.path(),
        vec![Arc::new(FailingSource)],
    ))
    .await
    .unwrap();
    let registry = TenantSkillManagerRegistry::new(factory);

    assert!(registry.for_tenant("preserved").await.is_err());

    assert_eq!(
        tokio::fs::read_to_string(data.join("keep.txt"))
            .await
            .unwrap(),
        "data"
    );
    assert_eq!(
        tokio::fs::read_to_string(cache.join("keep.txt"))
            .await
            .unwrap(),
        "cache"
    );
    assert!(!data.join("state.db").exists());
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

    assert_eq!(
        error.to_string(),
        "tenant skill manager initialization failed"
    );
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

fn tenant_config(
    root: &std::path::Path,
    sources: Vec<Arc<dyn SkillSource>>,
) -> TenantSkillManagerConfig {
    TenantSkillManagerConfig {
        data_root: root.join("data"),
        cache_root: root.join("cache"),
        sources,
        platform: PlatformId::Server,
        capabilities: CapabilitySet::server_runtime(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: "0.1.0".parse().unwrap(),
        management_policy: SkillManagementPolicy::owner_only(),
    }
}

struct FailFirstFactory {
    delegate: FilesystemTenantSkillManagerFactory,
    attempts: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl TenantSkillManagerFactory for FailFirstFactory {
    async fn create(&self, tenant_id: &str) -> anyhow::Result<TenantSkillRuntime> {
        if self.attempts.fetch_add(1, Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(40)).await;
            anyhow::bail!("injected tenant initialization failure");
        }
        self.delegate.create(tenant_id).await
    }
}

struct DelayedFactory {
    delegate: FilesystemTenantSkillManagerFactory,
    attempts: Arc<AtomicUsize>,
}

#[async_trait::async_trait]
impl TenantSkillManagerFactory for DelayedFactory {
    async fn create(&self, tenant_id: &str) -> anyhow::Result<TenantSkillRuntime> {
        self.attempts.fetch_add(1, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(100)).await;
        self.delegate.create(tenant_id).await
    }
}

struct FailingSource;

#[async_trait::async_trait]
impl SkillSource for FailingSource {
    fn layer(&self) -> SkillLayer {
        SkillLayer::Builtin
    }

    async fn discover(&self) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
        anyhow::bail!("injected discovery failure")
    }
}

async fn wait_for_attempt(attempts: &AtomicUsize) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while attempts.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
