use agent_runtime::{
    platform::{CapabilitySet, PlatformId},
    skill_bundle::{BundleSkillSource, SKILL_BUNDLE_MANIFEST_FILE},
    skill_manager::{SkillManager, SkillManagerConfig},
    skill_source::{DirectorySkillSource, ManagedSkillSource, SkillLayer, SkillSource},
    skill_state::SkillStateStore,
    skill_store::{SkillRevisionStore, SkillStorePaths},
    storage::Storage,
};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ManagedSkillsConfig {
    pub(super) app_data_root: PathBuf,
    pub(super) cache_root: PathBuf,
}

pub(super) struct LoadedSkillManager {
    pub(super) manager: SkillManager,
    pub(super) managed_store: Option<SkillRevisionStore>,
    #[cfg(test)]
    pub(super) managed_source: Option<ManagedSkillSource>,
}

pub(super) fn managed_skills_config_from_lookup<F>(
    lookup: F,
) -> anyhow::Result<Option<ManagedSkillsConfig>>
where
    F: Fn(&str) -> Option<std::ffi::OsString>,
{
    if lookup("GENERAL_AGENT_MANAGED_SKILLS").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Ok(None);
    }
    let required_root = |name: &str| -> anyhow::Result<PathBuf> {
        let value = lookup(name).ok_or_else(|| {
            anyhow::anyhow!("{name} is required when GENERAL_AGENT_MANAGED_SKILLS=1")
        })?;
        if value.is_empty() {
            anyhow::bail!("{name} cannot be empty when GENERAL_AGENT_MANAGED_SKILLS=1");
        }
        Ok(PathBuf::from(value))
    };
    Ok(Some(ManagedSkillsConfig {
        app_data_root: required_root("GENERAL_AGENT_APP_DATA_ROOT")?,
        cache_root: required_root("GENERAL_AGENT_CACHE_ROOT")?,
    }))
}

pub(super) async fn load_skill_manager(
    root: &Path,
    storage: Storage,
    managed_config: Option<ManagedSkillsConfig>,
) -> anyhow::Result<LoadedSkillManager> {
    let deferred_managed_startup = managed_config.is_some();
    let manifest = root.join(SKILL_BUNDLE_MANIFEST_FILE);
    let has_bundle_manifest = match tokio::fs::symlink_metadata(&manifest).await {
        Ok(_) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Err(error) => return Err(error.into()),
    };
    let builtin: Arc<dyn SkillSource> = if has_bundle_manifest {
        Arc::new(BundleSkillSource::open(root).await?)
    } else {
        Arc::new(DirectorySkillSource::new(SkillLayer::Builtin, root))
    };
    let mut sources = vec![builtin];
    let mut managed_store = None;
    #[cfg(test)]
    let mut managed_source = None;
    if let Some(config) = managed_config {
        let paths = SkillStorePaths::prepare(&config.app_data_root, &config.cache_root).await?;
        let store = SkillRevisionStore::new(paths, SkillStateStore::new(storage));
        let source = ManagedSkillSource::from_store(store.clone());
        sources.push(Arc::new(source.clone()));
        managed_store = Some(store);
        #[cfg(test)]
        {
            managed_source = Some(source);
        }
    }
    let config = SkillManagerConfig {
        sources,
        platform: PlatformId::Desktop,
        capabilities: CapabilitySet::desktop_runtime(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: env!("CARGO_PKG_VERSION").parse()?,
    };
    let manager = if deferred_managed_startup {
        SkillManager::new_deferred_managed(config).await?
    } else {
        SkillManager::new(config).await?
    };
    Ok(LoadedSkillManager {
        manager,
        managed_store,
        #[cfg(test)]
        managed_source,
    })
}
