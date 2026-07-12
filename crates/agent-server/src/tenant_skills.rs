use agent_runtime::platform::{CapabilitySet, PlatformId};
use agent_runtime::skill_management::OwnerSkillManagementService;
use agent_runtime::skill_manager::{SkillManager, SkillManagerConfig};
use agent_runtime::skill_package::SkillPackageId;
use agent_runtime::skill_policy::SkillManagementPolicy;
use agent_runtime::skill_source::{ManagedSkillSource, SkillSource};
use agent_runtime::skill_state::SkillStateStore;
use agent_runtime::skill_store::{SkillRevisionStore, SkillStorePaths};
use agent_runtime::storage::Storage;
use semver::Version;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use tokio::sync::{Notify, OnceCell};

pub const SINGLE_USER_TENANT_ID: &str = "local";

#[derive(Clone)]
pub struct TenantSkillRuntime {
    pub tenant_id: String,
    pub manager: SkillManager,
    pub management: OwnerSkillManagementService,
    pub state: SkillStateStore,
    pub revisions: SkillRevisionStore,
    pub storage: Storage,
    pub data_root: PathBuf,
    pub cache_root: PathBuf,
    pub database_path: PathBuf,
}

impl std::fmt::Debug for TenantSkillRuntime {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("TenantSkillRuntime")
            .field("tenant_id", &self.tenant_id)
            .field("data_root", &self.data_root)
            .field("cache_root", &self.cache_root)
            .field("database_path", &self.database_path)
            .finish_non_exhaustive()
    }
}

#[async_trait::async_trait]
pub trait TenantSkillManagerFactory: Send + Sync {
    async fn create(&self, tenant_id: &str) -> anyhow::Result<TenantSkillRuntime>;
}

#[derive(Clone)]
pub struct TenantSkillManagerRegistry {
    managers: TenantManagerCells,
    factory: Arc<dyn TenantSkillManagerFactory>,
}

struct TenantManagerEntry {
    runtime: OnceCell<Arc<TenantSkillRuntime>>,
    status: Mutex<TenantManagerStatus>,
    notify: Notify,
}

enum TenantManagerStatus {
    Initializing,
    Failed,
}

impl TenantManagerEntry {
    fn new() -> Self {
        Self {
            runtime: OnceCell::new(),
            status: Mutex::new(TenantManagerStatus::Initializing),
            notify: Notify::new(),
        }
    }
}

type TenantManagerCell = Arc<TenantManagerEntry>;
type TenantManagerCells = Arc<Mutex<HashMap<String, TenantManagerCell>>>;

impl TenantSkillManagerRegistry {
    pub fn new(factory: impl TenantSkillManagerFactory + 'static) -> Self {
        Self {
            managers: Arc::new(Mutex::new(HashMap::new())),
            factory: Arc::new(factory),
        }
    }

    pub async fn for_tenant(&self, tenant_id: &str) -> anyhow::Result<Arc<TenantSkillRuntime>> {
        let tenant_id = validate_tenant_id(tenant_id)?;
        let (cell, initialize) = {
            let mut managers = self
                .managers
                .lock()
                .expect("tenant skill registry lock poisoned");
            if let Some(cell) = managers.get(&tenant_id) {
                (cell.clone(), false)
            } else {
                let cell = Arc::new(TenantManagerEntry::new());
                managers.insert(tenant_id.clone(), cell.clone());
                (cell, true)
            }
        };
        if initialize {
            self.spawn_initialization(tenant_id, cell.clone());
        }
        loop {
            let notified = cell.notify.notified();
            if let Some(runtime) = cell.runtime.get() {
                return Ok(runtime.clone());
            }
            if matches!(
                *cell
                    .status
                    .lock()
                    .expect("tenant manager entry lock poisoned"),
                TenantManagerStatus::Failed
            ) {
                anyhow::bail!("tenant skill manager initialization failed");
            }
            notified.await;
        }
    }

    fn spawn_initialization(&self, tenant_id: String, cell: TenantManagerCell) {
        let registry = self.clone();
        tokio::spawn(async move {
            match registry.factory.create(&tenant_id).await {
                Ok(runtime) => {
                    let published = cell.runtime.set(Arc::new(runtime)).is_ok();
                    debug_assert!(published, "tenant runtime initialized more than once");
                }
                Err(_) => {
                    *cell
                        .status
                        .lock()
                        .expect("tenant manager entry lock poisoned") = TenantManagerStatus::Failed;
                    let mut managers = registry
                        .managers
                        .lock()
                        .expect("tenant skill registry lock poisoned");
                    if managers
                        .get(&tenant_id)
                        .is_some_and(|current| Arc::ptr_eq(current, &cell))
                    {
                        managers.remove(&tenant_id);
                    }
                }
            }
            cell.notify.notify_waiters();
        });
    }

    pub fn manager_count(&self) -> usize {
        self.managers
            .lock()
            .expect("tenant skill registry lock poisoned")
            .values()
            .filter(|cell| cell.runtime.get().is_some())
            .count()
    }

    pub fn entry_count(&self) -> usize {
        self.managers
            .lock()
            .expect("tenant skill registry lock poisoned")
            .len()
    }
}

pub fn validate_tenant_id(value: &str) -> anyhow::Result<String> {
    let bytes = value.as_bytes();
    let canonical = !bytes.is_empty()
        && bytes.len() <= 63
        && canonical_tenant_byte(bytes[0])
        && canonical_tenant_byte(bytes[bytes.len() - 1])
        && bytes
            .iter()
            .all(|byte| canonical_tenant_byte(*byte) || *byte == b'-');
    if !canonical {
        anyhow::bail!("tenant id must be canonical lowercase ASCII");
    }
    Ok(value.to_string())
}

fn canonical_tenant_byte(byte: u8) -> bool {
    byte.is_ascii_lowercase() || byte.is_ascii_digit()
}

#[derive(Clone)]
pub struct TenantSkillManagerConfig {
    pub data_root: PathBuf,
    pub cache_root: PathBuf,
    pub sources: Vec<Arc<dyn SkillSource>>,
    pub platform: PlatformId,
    pub capabilities: CapabilitySet,
    pub protected_packages: Vec<SkillPackageId>,
    pub allowed_overrides: Vec<SkillPackageId>,
    pub runtime_version: Version,
    pub management_policy: SkillManagementPolicy,
}

#[derive(Clone)]
pub struct FilesystemTenantSkillManagerFactory {
    config: TenantSkillManagerConfig,
    data_tenants: PathBuf,
    cache_tenants: PathBuf,
}

impl FilesystemTenantSkillManagerFactory {
    pub async fn new(mut config: TenantSkillManagerConfig) -> anyhow::Result<Self> {
        config.data_root = prepare_real_directory(&config.data_root).await?;
        config.cache_root = prepare_real_directory(&config.cache_root).await?;
        let data_tenants = prepare_real_directory(&config.data_root.join("tenants")).await?;
        let cache_tenants = prepare_real_directory(&config.cache_root.join("tenants")).await?;
        Ok(Self {
            config,
            data_tenants,
            cache_tenants,
        })
    }
}

#[async_trait::async_trait]
impl TenantSkillManagerFactory for FilesystemTenantSkillManagerFactory {
    async fn create(&self, tenant_id: &str) -> anyhow::Result<TenantSkillRuntime> {
        let tenant_id = validate_tenant_id(tenant_id)?;
        let data = prepare_real_tenant_child(&self.data_tenants, &tenant_id).await?;
        let cache = match prepare_real_tenant_child(&self.cache_tenants, &tenant_id).await {
            Ok(cache) => cache,
            Err(error) => {
                cleanup_created_directory(&data).await;
                return Err(error);
            }
        };
        let cleanup = TenantInitializationPaths::capture(data, cache).await?;
        let result = self.create_runtime(tenant_id, &cleanup).await;
        if result.is_err() {
            cleanup.cleanup().await;
        }
        result
    }
}

impl FilesystemTenantSkillManagerFactory {
    async fn create_runtime(
        &self,
        tenant_id: String,
        cleanup: &TenantInitializationPaths,
    ) -> anyhow::Result<TenantSkillRuntime> {
        let data_root = cleanup.data.path.clone();
        let cache_root = cleanup.cache.path.clone();
        let database_path = data_root.join("state.db");
        reject_symlink_or_non_file_if_present(&database_path).await?;
        let storage =
            Storage::connect(&format!("sqlite://{}?mode=rwc", database_path.display())).await?;
        reject_symlink_or_non_file_if_present(&database_path).await?;
        ensure_parent_identity(&data_root, &database_path).await?;
        let state = SkillStateStore::new(storage.clone());
        let paths =
            SkillStorePaths::prepare(&data_root.join("app"), &cache_root.join("cache")).await?;
        let revisions = SkillRevisionStore::new(paths, state.clone());
        let mut sources = self.config.sources.clone();
        sources.push(Arc::new(ManagedSkillSource::from_store(revisions.clone())));
        let manager = SkillManager::new_deferred_managed(SkillManagerConfig {
            sources,
            platform: self.config.platform,
            capabilities: self.config.capabilities.clone(),
            protected_packages: self.config.protected_packages.clone(),
            allowed_overrides: self.config.allowed_overrides.clone(),
            runtime_version: self.config.runtime_version.clone(),
        })
        .await?;
        let management = OwnerSkillManagementService::new(
            manager.clone(),
            revisions.clone(),
            state.clone(),
            self.config.management_policy.clone(),
        );
        manager.startup_reconcile().await?;
        Ok(TenantSkillRuntime {
            tenant_id,
            manager,
            management,
            state,
            revisions,
            storage,
            data_root,
            cache_root,
            database_path,
        })
    }
}

async fn prepare_real_directory(path: &Path) -> anyhow::Result<PathBuf> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => ensure_real_directory(path, &metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::create_dir_all(path).await?;
            let metadata = tokio::fs::symlink_metadata(path).await?;
            ensure_real_directory(path, &metadata)?;
        }
        Err(error) => return Err(error.into()),
    }
    Ok(tokio::fs::canonicalize(path).await?)
}

struct PreparedTenantDirectory {
    path: PathBuf,
    created: bool,
}

async fn prepare_real_tenant_child(
    parent: &Path,
    tenant_id: &str,
) -> anyhow::Result<PreparedTenantDirectory> {
    let child = parent.join(tenant_id);
    let mut created = false;
    match tokio::fs::symlink_metadata(&child).await {
        Ok(metadata) => ensure_real_directory(&child, &metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match tokio::fs::create_dir(&child).await {
                Ok(()) => created = true,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error.into()),
            }
            let metadata = tokio::fs::symlink_metadata(&child).await?;
            ensure_real_directory(&child, &metadata)?;
        }
        Err(error) => return Err(error.into()),
    }
    let canonical = tokio::fs::canonicalize(&child).await?;
    let expected = parent.join(tenant_id);
    anyhow::ensure!(
        canonical == expected,
        "tenant root has a canonical alias instead of the requested tenant id"
    );
    Ok(PreparedTenantDirectory {
        path: canonical,
        created,
    })
}

struct TrackedPath {
    path: PathBuf,
    existed: bool,
}

struct TenantInitializationPaths {
    data: PreparedTenantDirectory,
    cache: PreparedTenantDirectory,
    files: Vec<TrackedPath>,
    directories: Vec<TrackedPath>,
}

impl TenantInitializationPaths {
    async fn capture(
        data: PreparedTenantDirectory,
        cache: PreparedTenantDirectory,
    ) -> anyhow::Result<Self> {
        let database = data.path.join("state.db");
        let files = vec![
            track_path(database.clone()).await?,
            track_path(data.path.join("state.db-wal")).await?,
            track_path(data.path.join("state.db-shm")).await?,
        ];
        let directories = [
            data.path.join("app"),
            data.path.join("app/managed-skills"),
            data.path.join("app/managed-skills/.locks"),
            data.path.join("app/skill-quarantine"),
            cache.path.join("cache"),
            cache.path.join("cache/skill-staging"),
        ];
        let mut tracked_directories = Vec::new();
        for path in directories {
            tracked_directories.push(track_path(path).await?);
        }
        Ok(Self {
            data,
            cache,
            files,
            directories: tracked_directories,
        })
    }

    async fn cleanup(&self) {
        for tracked in &self.files {
            if !tracked.existed {
                remove_created_file(&tracked.path).await;
            }
        }
        for tracked in self.directories.iter().rev() {
            if !tracked.existed {
                remove_created_empty_directory(&tracked.path).await;
            }
        }
        cleanup_created_directory(&self.cache).await;
        cleanup_created_directory(&self.data).await;
    }
}

async fn track_path(path: PathBuf) -> anyhow::Result<TrackedPath> {
    let existed = match tokio::fs::symlink_metadata(&path).await {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    Ok(TrackedPath { path, existed })
}

async fn cleanup_created_directory(directory: &PreparedTenantDirectory) {
    if directory.created {
        remove_created_empty_directory(&directory.path).await;
    }
}

async fn remove_created_file(path: &Path) {
    if let Err(error) = tokio::fs::remove_file(path).await
        && error.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("failed to clean tenant initialization file");
    }
}

async fn remove_created_empty_directory(path: &Path) {
    if let Err(error) = tokio::fs::remove_dir(path).await
        && !matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
        )
    {
        tracing::warn!("failed to clean tenant initialization directory");
    }
}

fn ensure_real_directory(path: &Path, metadata: &std::fs::Metadata) -> anyhow::Result<()> {
    anyhow::ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "tenant storage path must be a real directory: {}",
        path.display()
    );
    Ok(())
}

async fn reject_symlink_or_non_file_if_present(path: &Path) -> anyhow::Result<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => anyhow::ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "tenant database path must be a real file"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

async fn ensure_parent_identity(parent: &Path, child: &Path) -> anyhow::Result<()> {
    let canonical_parent = tokio::fs::canonicalize(parent).await?;
    let actual_parent = child
        .parent()
        .ok_or_else(|| anyhow::anyhow!("tenant database has no parent"))?;
    anyhow::ensure!(
        tokio::fs::canonicalize(actual_parent).await? == canonical_parent,
        "tenant database parent changed during initialization"
    );
    Ok(())
}
