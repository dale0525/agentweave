use crate::platform::{CapabilitySet, PlatformId};
use crate::skill::SkillRegistry;
use crate::skill_catalog::SkillCatalog;
use crate::skill_package::SkillPackageId;
use crate::skill_resolver::{SkillResolutionInput, SkillResolver};
use crate::skill_snapshot::SkillSnapshot;
use crate::skill_source::SkillSource;
use anyhow::Context;
use semver::Version;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, RwLock};
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct SkillManager {
    inner: Arc<SkillManagerInner>,
}

struct SkillManagerInner {
    mode: SkillManagerMode,
    runtime_context: Option<SkillRuntimeContext>,
    current: RwLock<Arc<SkillSnapshot>>,
    reload_lock: Mutex<()>,
}

enum SkillManagerMode {
    Dynamic(SkillManagerConfig),
    Static,
}

#[derive(Clone)]
pub struct SkillManagerConfig {
    pub sources: Vec<Arc<dyn SkillSource>>,
    pub platform: PlatformId,
    pub capabilities: CapabilitySet,
    pub protected_packages: Vec<SkillPackageId>,
    pub allowed_overrides: Vec<SkillPackageId>,
    pub runtime_version: Version,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkillRuntimeContext {
    platform: PlatformId,
    capabilities: CapabilitySet,
}

impl SkillRuntimeContext {
    fn new(platform: PlatformId, capabilities: CapabilitySet) -> Self {
        Self {
            platform,
            capabilities,
        }
    }

    pub fn platform(&self) -> PlatformId {
        self.platform
    }

    pub fn capabilities(&self) -> &CapabilitySet {
        &self.capabilities
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct SkillReloadReport {
    pub previous_generation: u64,
    pub active_generation: u64,
    pub active_packages: usize,
    pub inactive_packages: usize,
}

impl SkillManager {
    pub async fn new(config: SkillManagerConfig) -> anyhow::Result<Self> {
        let initial = Arc::new(build_snapshot(&config, 1).await?);
        let runtime_context =
            SkillRuntimeContext::new(config.platform, config.capabilities.clone());
        Ok(Self::with_mode(
            initial,
            SkillManagerMode::Dynamic(config),
            Some(runtime_context),
        ))
    }

    pub fn from_registry_and_catalog(registry: SkillRegistry, catalog: SkillCatalog) -> Self {
        let snapshot = SkillSnapshot::from_registry_and_catalog(1, registry, catalog);
        Self::with_mode(Arc::new(snapshot), SkillManagerMode::Static, None)
    }

    pub fn from_registry_and_catalog_with_context(
        registry: SkillRegistry,
        catalog: SkillCatalog,
        platform: PlatformId,
        capabilities: CapabilitySet,
    ) -> Self {
        let snapshot = SkillSnapshot::from_registry_and_catalog(1, registry, catalog)
            .with_platform_capabilities(platform, capabilities.clone());
        Self::with_mode(
            Arc::new(snapshot),
            SkillManagerMode::Static,
            Some(SkillRuntimeContext::new(platform, capabilities)),
        )
    }

    pub fn current_snapshot(&self) -> Arc<SkillSnapshot> {
        self.inner
            .current
            .read()
            .expect("skill snapshot lock poisoned")
            .clone()
    }

    pub fn runtime_context(&self) -> Option<&SkillRuntimeContext> {
        self.inner.runtime_context.as_ref()
    }

    pub async fn reload(&self) -> anyhow::Result<SkillReloadReport> {
        let (report, ()) = self.reload_with_pre_publish(|_| async { Ok(()) }).await?;
        Ok(report)
    }

    /// Builds a candidate under the reload lock and publishes it only after preparation succeeds.
    /// The callback must not call either reload method because the reload lock is non-reentrant.
    pub async fn reload_with_pre_publish<T, F, Fut>(
        &self,
        pre_publish: F,
    ) -> anyhow::Result<(SkillReloadReport, T)>
    where
        F: FnOnce(Arc<SkillSnapshot>) -> Fut,
        Fut: Future<Output = anyhow::Result<T>>,
    {
        let _guard = self.inner.reload_lock.lock().await;
        let SkillManagerMode::Dynamic(config) = &self.inner.mode else {
            anyhow::bail!("static skill manager cannot reload");
        };
        let previous = self.current_snapshot();
        let generation = previous
            .generation()
            .checked_add(1)
            .context("skill snapshot generation overflow")?;
        let candidate = Arc::new(build_snapshot(config, generation).await?);
        let prepared = pre_publish(candidate.clone()).await?;
        let report = SkillReloadReport {
            previous_generation: previous.generation(),
            active_generation: candidate.generation(),
            active_packages: candidate.packages().len(),
            inactive_packages: candidate.inactive().len(),
        };
        *self
            .inner
            .current
            .write()
            .expect("skill snapshot lock poisoned") = candidate;
        Ok((report, prepared))
    }

    fn with_mode(
        current: Arc<SkillSnapshot>,
        mode: SkillManagerMode,
        runtime_context: Option<SkillRuntimeContext>,
    ) -> Self {
        Self {
            inner: Arc::new(SkillManagerInner {
                mode,
                runtime_context,
                current: RwLock::new(current),
                reload_lock: Mutex::new(()),
            }),
        }
    }
}

impl fmt::Debug for SkillManager {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SkillManager")
            .field("generation", &self.current_snapshot().generation())
            .finish_non_exhaustive()
    }
}

async fn build_snapshot(
    config: &SkillManagerConfig,
    generation: u64,
) -> anyhow::Result<SkillSnapshot> {
    let mut packages = Vec::new();
    for source in &config.sources {
        let layer = source.layer();
        let discovered = source.discover().await?;
        if let Some(package) = discovered.iter().find(|package| package.layer != layer) {
            anyhow::bail!(
                "skill source layer {:?} returned package {} with source layer {:?}",
                layer,
                package.descriptor.id.as_str(),
                package.layer
            );
        }
        packages.extend(discovered);
    }
    let resolved = SkillResolver::resolve(SkillResolutionInput {
        packages,
        platform: config.platform,
        capabilities: config.capabilities.clone(),
        protected_packages: config.protected_packages.clone(),
        allowed_overrides: config.allowed_overrides.clone(),
        runtime_version: config.runtime_version.clone(),
    })?;
    Ok(SkillSnapshot::build(generation, resolved)
        .await?
        .with_platform_capabilities(config.platform, config.capabilities.clone()))
}
