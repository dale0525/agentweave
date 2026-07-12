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
use tokio::sync::OnceCell;

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

type TenantManagerCell = Arc<OnceCell<Arc<TenantSkillRuntime>>>;
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
        let cell = {
            let mut managers = self
                .managers
                .lock()
                .expect("tenant skill registry lock poisoned");
            managers
                .entry(tenant_id.clone())
                .or_insert_with(|| Arc::new(OnceCell::new()))
                .clone()
        };
        cell.get_or_try_init(|| async {
            self.factory
                .create(&tenant_id)
                .await
                .map(Arc::new)
                .map_err(|error| {
                    anyhow::anyhow!("tenant skill manager initialization failed: {error}")
                })
        })
        .await
        .cloned()
    }

    pub fn manager_count(&self) -> usize {
        self.managers
            .lock()
            .expect("tenant skill registry lock poisoned")
            .values()
            .filter(|cell| cell.get().is_some())
            .count()
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
        let data_root = prepare_real_tenant_child(&self.data_tenants, &tenant_id).await?;
        let cache_root = prepare_real_tenant_child(&self.cache_tenants, &tenant_id).await?;
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

async fn prepare_real_tenant_child(parent: &Path, tenant_id: &str) -> anyhow::Result<PathBuf> {
    let child = parent.join(tenant_id);
    match tokio::fs::symlink_metadata(&child).await {
        Ok(metadata) => ensure_real_directory(&child, &metadata)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match tokio::fs::create_dir(&child).await {
                Ok(()) => {}
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
    Ok(canonical)
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
