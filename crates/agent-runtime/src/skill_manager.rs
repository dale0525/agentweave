use crate::platform::{CapabilitySet, PlatformId};
use crate::skill::SkillRegistry;
use crate::skill_catalog::SkillCatalog;
use crate::skill_package::SkillPackageId;
use crate::skill_resolver::{SkillResolutionInput, SkillResolver};
use crate::skill_snapshot::SkillSnapshot;
use crate::skill_source::{DiscoveredSkillPackage, SkillLayer, SkillSource};
use anyhow::Context;
use semver::Version;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, RwLock};
use tokio::sync::{Mutex, OwnedMutexGuard};

#[derive(Clone)]
pub struct SkillManager {
    inner: Arc<SkillManagerInner>,
}

struct SkillManagerInner {
    mode: SkillManagerMode,
    runtime_context: Option<SkillRuntimeContext>,
    current: RwLock<Arc<SkillSnapshot>>,
    reload_lock: Arc<Mutex<()>>,
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

#[derive(Debug)]
pub(crate) struct SkillSnapshotPreview {
    pub base: Arc<SkillSnapshot>,
    pub candidate: Arc<SkillSnapshot>,
}

pub(crate) struct SkillPublicationGuard {
    manager: SkillManager,
    previous: Arc<SkillSnapshot>,
    _lock: OwnedMutexGuard<()>,
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

    pub(crate) fn validation_runtime(&self) -> (PlatformId, CapabilitySet, Version) {
        match &self.inner.mode {
            SkillManagerMode::Dynamic(config) => (
                config.platform,
                config.capabilities.clone(),
                config.runtime_version.clone(),
            ),
            SkillManagerMode::Static => {
                let (platform, capabilities) = self.inner.runtime_context.as_ref().map_or_else(
                    || {
                        (
                            PlatformId::Server,
                            CapabilitySet::from_names(Vec::<String>::new()),
                        )
                    },
                    |context| (context.platform, context.capabilities.clone()),
                );
                let version = env!("CARGO_PKG_VERSION")
                    .parse()
                    .expect("crate package version must be valid semver");
                (platform, capabilities, version)
            }
        }
    }

    pub async fn reload(&self) -> anyhow::Result<SkillReloadReport> {
        let (report, ()) = self.reload_with_pre_publish(|_| async { Ok(()) }).await?;
        Ok(report)
    }

    pub(crate) async fn preview_candidate(
        &self,
        candidate: DiscoveredSkillPackage,
    ) -> anyhow::Result<SkillSnapshotPreview> {
        let _guard = self.inner.reload_lock.lock().await;
        let SkillManagerMode::Dynamic(config) = &self.inner.mode else {
            anyhow::bail!("static skill manager cannot preview a managed candidate");
        };
        if candidate.layer != SkillLayer::Managed {
            anyhow::bail!("skill preview candidate must use the managed layer");
        }
        let base = self.current_snapshot();
        let candidate_snapshot =
            Arc::new(build_snapshot_with_candidate(config, base.generation(), candidate).await?);
        Ok(SkillSnapshotPreview {
            base,
            candidate: candidate_snapshot,
        })
    }

    pub(crate) async fn begin_publication(&self) -> anyhow::Result<SkillPublicationGuard> {
        let lock = self.inner.reload_lock.clone().lock_owned().await;
        if !matches!(self.inner.mode, SkillManagerMode::Dynamic(_)) {
            anyhow::bail!("static skill manager cannot publish");
        }
        Ok(SkillPublicationGuard {
            manager: self.clone(),
            previous: self.current_snapshot(),
            _lock: lock,
        })
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
                reload_lock: Arc::new(Mutex::new(())),
            }),
        }
    }
}

impl SkillPublicationGuard {
    pub(crate) fn base_generation(&self) -> u64 {
        self.previous.generation()
    }

    pub(crate) async fn build_candidate(
        &self,
        candidate: DiscoveredSkillPackage,
    ) -> anyhow::Result<Arc<SkillSnapshot>> {
        let SkillManagerMode::Dynamic(config) = &self.manager.inner.mode else {
            anyhow::bail!("static skill manager cannot publish");
        };
        let generation = self
            .previous
            .generation()
            .checked_add(1)
            .context("skill snapshot generation overflow")?;
        Ok(Arc::new(
            build_snapshot_with_candidate(config, generation, candidate).await?,
        ))
    }

    pub(crate) async fn has_builtin(&self, package_id: &SkillPackageId) -> anyhow::Result<bool> {
        let SkillManagerMode::Dynamic(config) = &self.manager.inner.mode else {
            anyhow::bail!("static skill manager cannot inspect publication sources");
        };
        Ok(discover_packages(config).await?.into_iter().any(|package| {
            package.layer == SkillLayer::Builtin && package.descriptor.id == *package_id
        }))
    }

    pub(crate) fn publish(self, candidate: Arc<SkillSnapshot>) -> SkillReloadReport {
        let report = SkillReloadReport {
            previous_generation: self.previous.generation(),
            active_generation: candidate.generation(),
            active_packages: candidate.packages().len(),
            inactive_packages: candidate.inactive().len(),
        };
        *self
            .manager
            .inner
            .current
            .write()
            .expect("skill snapshot lock poisoned") = candidate;
        report
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
    let packages = discover_packages(config).await?;
    build_snapshot_from_packages(config, generation, packages).await
}

async fn build_snapshot_with_candidate(
    config: &SkillManagerConfig,
    generation: u64,
    candidate: DiscoveredSkillPackage,
) -> anyhow::Result<SkillSnapshot> {
    let candidate_id = candidate.descriptor.id.clone();
    let mut packages = discover_packages(config).await?;
    packages.retain(|package| {
        !(package.layer == SkillLayer::Managed && package.descriptor.id == candidate_id)
    });
    packages.push(candidate);
    build_snapshot_from_packages(config, generation, packages).await
}

async fn discover_packages(
    config: &SkillManagerConfig,
) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
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
    Ok(packages)
}

async fn build_snapshot_from_packages(
    config: &SkillManagerConfig,
    generation: u64,
    packages: Vec<DiscoveredSkillPackage>,
) -> anyhow::Result<SkillSnapshot> {
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
