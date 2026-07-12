use agent_runtime::{
    platform::{CapabilitySet, PlatformId},
    skill_bundle::{
        BundleSkillSource, SKILL_BUNDLE_CURRENT_FILE, SKILL_BUNDLE_GENERATIONS_DIR,
        SKILL_BUNDLE_LOCK_FILE, SKILL_BUNDLE_MANIFEST_FILE,
    },
    skill_manager::{SkillManager, SkillManagerConfig},
    skill_source::{DirectorySkillSource, ManagedSkillSource, SkillLayer, SkillSource},
    skill_state::SkillStateStore,
    skill_store::{SkillRevisionStore, SkillStorePaths},
    storage::Storage,
};
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::Arc;

const BUNDLE_SOURCE_MODE: &[u8] = b"bundle-v1\n";

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
    let builtin: Arc<dyn SkillSource> = if has_bundle_evidence(root).await? {
        let source = BundleSkillSource::open(root).await?;
        persist_bundle_source_mode(root).await?;
        Arc::new(source)
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

async fn has_bundle_evidence(root: &Path) -> anyhow::Result<bool> {
    if has_persisted_bundle_source_mode(root).await? {
        return Ok(true);
    }
    for entry in [
        SKILL_BUNDLE_MANIFEST_FILE,
        SKILL_BUNDLE_LOCK_FILE,
        SKILL_BUNDLE_CURRENT_FILE,
        SKILL_BUNDLE_GENERATIONS_DIR,
    ] {
        match tokio::fs::symlink_metadata(root.join(entry)).await {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(false)
}

async fn has_persisted_bundle_source_mode(root: &Path) -> anyhow::Result<bool> {
    let marker = bundle_source_mode_path(root)?;
    match tokio::fs::symlink_metadata(&marker).await {
        Ok(metadata) => {
            anyhow::ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink(),
                "builtin skill source mode marker must be a regular file: {}",
                marker.display()
            );
            let bytes = tokio::fs::read(&marker).await?;
            anyhow::ensure!(
                bytes == BUNDLE_SOURCE_MODE,
                "unsupported builtin skill source mode marker: {}",
                marker.display()
            );
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}

async fn persist_bundle_source_mode(root: &Path) -> anyhow::Result<()> {
    let marker = bundle_source_mode_path(root)?;
    if has_persisted_bundle_source_mode(root).await? {
        return Ok(());
    }
    let temporary = marker.with_extension(format!("tmp-{}", uuid::Uuid::new_v4()));
    let result = async {
        let mut file = tokio::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .await?;
        use tokio::io::AsyncWriteExt;
        file.write_all(BUNDLE_SOURCE_MODE).await?;
        file.sync_all().await?;
        drop(file);
        if let Err(error) = tokio::fs::rename(&temporary, &marker).await
            && !has_persisted_bundle_source_mode(root).await?
        {
            return Err(error.into());
        }
        Ok::<(), anyhow::Error>(())
    }
    .await;
    let _ = tokio::fs::remove_file(&temporary).await;
    result
}

fn bundle_source_mode_path(root: &Path) -> anyhow::Result<PathBuf> {
    let absolute = if root.is_absolute() {
        root.to_path_buf()
    } else {
        std::env::current_dir()?.join(root)
    };
    let name = absolute
        .file_name()
        .context("builtin skills root has no file name")?
        .to_string_lossy();
    let parent = absolute
        .parent()
        .context("builtin skills root has no parent")?;
    Ok(parent.join(format!(".{name}.general-agent-source-mode")))
}
