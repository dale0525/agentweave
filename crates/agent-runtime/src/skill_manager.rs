use crate::platform::{CapabilitySet, PlatformId};
use crate::skill::SkillRegistry;
use crate::skill_catalog::SkillCatalog;
use crate::skill_package::SkillPackageId;
use crate::skill_resolver::{SkillResolutionInput, SkillResolver};
use crate::skill_snapshot::{SkillSnapshot, SkillSnapshotLease};
use crate::skill_source::{DiscoveredSkillPackage, SkillLayer, SkillSource};
use anyhow::Context;
use semver::Version;
use std::fmt;
use std::future::Future;
use std::sync::{Arc, RwLock, Weak};
use tokio::sync::{Mutex, OwnedMutexGuard};

#[path = "skill_manager_circuit.rs"]
mod circuit;
#[path = "skill_manager_startup.rs"]
mod startup;

#[derive(Clone)]
pub struct SkillManager {
    inner: Arc<SkillManagerInner>,
}

struct SkillManagerInner {
    mode: SkillManagerMode,
    runtime_context: Option<SkillRuntimeContext>,
    current: RwLock<Arc<SkillSnapshot>>,
    reload_lock: Arc<Mutex<()>>,
    managed_runtime: RwLock<Option<ManagedRuntimeBackend>>,
    live_snapshots: std::sync::Mutex<Vec<Weak<SkillSnapshot>>>,
}

#[derive(Clone)]
pub(crate) struct ManagedRuntimeBackend {
    pub(crate) revisions: crate::skill_store::SkillRevisionStore,
    pub(crate) state: crate::skill_state::SkillStateStore,
    pub(crate) events: tokio::sync::broadcast::Sender<crate::events::RuntimeEvent>,
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

pub(crate) struct SkillPublicationSourceView {
    packages: Vec<DiscoveredSkillPackage>,
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

    pub async fn new_deferred_managed(config: SkillManagerConfig) -> anyhow::Result<Self> {
        let packages = discover_non_managed_packages_read_only(&config).await?;
        let initial = Arc::new(build_snapshot_from_packages(&config, 1, packages).await?);
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

    pub fn lease_snapshot(&self) -> SkillSnapshotLease {
        let snapshot = self.current_snapshot();
        let mut live = self
            .inner
            .live_snapshots
            .lock()
            .expect("skill snapshot lease lock poisoned");
        live.retain(|entry| entry.strong_count() > 0);
        if !live
            .iter()
            .any(|entry| entry.ptr_eq(&Arc::downgrade(&snapshot)))
        {
            live.push(Arc::downgrade(&snapshot));
        }
        SkillSnapshotLease::new(snapshot)
    }

    pub(crate) fn live_snapshot_protections(
        &self,
    ) -> (
        std::collections::BTreeSet<u64>,
        std::collections::BTreeSet<String>,
    ) {
        let mut generations = std::collections::BTreeSet::new();
        let mut revisions = std::collections::BTreeSet::new();
        let mut live = self
            .inner
            .live_snapshots
            .lock()
            .expect("skill snapshot lease lock poisoned");
        live.retain(|entry| entry.strong_count() > 0);
        for snapshot in live.iter().filter_map(Weak::upgrade) {
            generations.insert(snapshot.generation());
            for resolved in snapshot.packages().iter().chain(snapshot.inactive()) {
                if let Some(revision_id) = resolved
                    .package
                    .verified_content
                    .as_ref()
                    .and_then(|content| content.execution_binding.as_ref())
                    .map(|binding| binding.revision_id.clone())
                {
                    revisions.insert(revision_id);
                }
            }
        }
        (generations, revisions)
    }

    pub(crate) fn bind_managed_runtime(
        &self,
        revisions: crate::skill_store::SkillRevisionStore,
        events: tokio::sync::broadcast::Sender<crate::events::RuntimeEvent>,
    ) {
        if !matches!(self.inner.mode, SkillManagerMode::Dynamic(_)) {
            return;
        }
        let state = revisions.state_store();
        *self
            .inner
            .managed_runtime
            .write()
            .expect("managed skill runtime lock poisoned") = Some(ManagedRuntimeBackend {
            revisions,
            state,
            events,
        });
    }

    pub(crate) fn managed_runtime(&self) -> anyhow::Result<ManagedRuntimeBackend> {
        if !matches!(self.inner.mode, SkillManagerMode::Dynamic(_)) {
            anyhow::bail!("static skill manager has no managed recovery runtime");
        }
        self.inner
            .managed_runtime
            .read()
            .expect("managed skill runtime lock poisoned")
            .clone()
            .context("managed skill runtime is not bound")
    }

    pub async fn startup_reconcile(
        &self,
    ) -> anyhow::Result<crate::skill_recovery::SkillRecoveryReport> {
        let _guard = self.inner.reload_lock.lock().await;
        let backend = self.managed_runtime()?;
        let mut maintenance_diagnostics = crate::skill_recovery::reconcile_startup_residue(
            &backend,
            self.current_snapshot().generation(),
        )
        .await?;
        let last_good_record = backend
            .state
            .snapshot_with_status(crate::skill_state::SkillSnapshotStatus::LastKnownGood)
            .await?;
        let last_good = if let Some(record) = &last_good_record {
            match self.rebuild_persisted_snapshot(&backend, record).await {
                Ok(snapshot) => Some(snapshot),
                Err(_) => {
                    let key = format!("invalid-lkg:{}:rebuild", record.generation);
                    backend
                        .state
                        .record_maintenance_diagnostic_once(
                            &key,
                            None,
                            "snapshot",
                            "invalid_last_known_good_snapshot",
                            serde_json::json!({
                                "generation": record.generation,
                                "phase": "rebuild"
                            }),
                        )
                        .await?;
                    maintenance_diagnostics += 1;
                    None
                }
            }
        } else {
            None
        };

        let active_record = backend
            .state
            .snapshot_with_status(crate::skill_state::SkillSnapshotStatus::Active)
            .await?;
        if let Some(record) = &active_record {
            if let Some(candidate) =
                circuit::expired_circuit_recovery_candidate(config_for(self)?, record, &backend)
                    .await?
            {
                backend
                    .state
                    .persist_recovery_candidate(
                        record,
                        candidate.generation(),
                        &crate::skill_recovery::snapshot_members(&candidate),
                    )
                    .await?;
                *self
                    .inner
                    .current
                    .write()
                    .expect("skill snapshot lock poisoned") = candidate.clone();
                let _ = backend
                    .events
                    .send(crate::events::RuntimeEvent::SkillRecoveryCompleted {
                        status: crate::skill_recovery::RecoveryStatus::NewSnapshotPublished,
                        generation: candidate.generation(),
                    });
                return Ok(crate::skill_recovery::SkillRecoveryReport {
                    status: crate::skill_recovery::RecoveryStatus::NewSnapshotPublished,
                    generation: candidate.generation(),
                    quarantined_revisions: Vec::new(),
                    maintenance_diagnostics,
                });
            }
            match self.rebuild_persisted_snapshot(&backend, record).await {
                Ok(snapshot) => {
                    *self
                        .inner
                        .current
                        .write()
                        .expect("skill snapshot lock poisoned") = snapshot.clone();
                    let _ =
                        backend
                            .events
                            .send(crate::events::RuntimeEvent::SkillRecoveryCompleted {
                                status: crate::skill_recovery::RecoveryStatus::CurrentSnapshotValid,
                                generation: snapshot.generation(),
                            });
                    return Ok(crate::skill_recovery::SkillRecoveryReport {
                        status: crate::skill_recovery::RecoveryStatus::CurrentSnapshotValid,
                        generation: snapshot.generation(),
                        quarantined_revisions: Vec::new(),
                        maintenance_diagnostics,
                    });
                }
                Err(_) if last_good.is_some() => {
                    let quarantined =
                        startup::quarantine_invalid_snapshot_members(&backend, record).await;
                    maintenance_diagnostics += quarantined.failures;
                    let restored = last_good.expect("last-known-good checked above");
                    let members = crate::skill_recovery::parse_snapshot_members(
                        last_good_record
                            .as_ref()
                            .expect("last-known-good record checked above")
                            .members_json
                            .clone(),
                    )?;
                    backend
                        .state
                        .restore_snapshot_as_active(
                            record,
                            last_good_record
                                .as_ref()
                                .expect("last-known-good record checked above"),
                            &members,
                        )
                        .await?;
                    *self
                        .inner
                        .current
                        .write()
                        .expect("skill snapshot lock poisoned") = restored.clone();
                    let _ =
                        backend
                            .events
                            .send(crate::events::RuntimeEvent::SkillRecoveryCompleted {
                                status:
                                    crate::skill_recovery::RecoveryStatus::LastKnownGoodRestored,
                                generation: restored.generation(),
                            });
                    return Ok(crate::skill_recovery::SkillRecoveryReport {
                        status: crate::skill_recovery::RecoveryStatus::LastKnownGoodRestored,
                        generation: restored.generation(),
                        quarantined_revisions: quarantined.revisions,
                        maintenance_diagnostics,
                    });
                }
                Err(_) => {
                    let quarantined =
                        startup::quarantine_invalid_snapshot_members(&backend, record).await;
                    maintenance_diagnostics += quarantined.failures;
                    let SkillManagerMode::Dynamic(config) = &self.inner.mode else {
                        unreachable!("managed runtime cannot be bound to a static manager")
                    };
                    let generation = record
                        .generation
                        .checked_add(1)
                        .context("skill snapshot generation overflow")?;
                    let candidate = Arc::new(
                        build_snapshot_with_runtime(config, generation, Some(&backend)).await?,
                    );
                    backend
                        .state
                        .persist_recovery_candidate(
                            record,
                            candidate.generation(),
                            &crate::skill_recovery::snapshot_members(&candidate),
                        )
                        .await?;
                    *self
                        .inner
                        .current
                        .write()
                        .expect("skill snapshot lock poisoned") = candidate.clone();
                    let _ =
                        backend
                            .events
                            .send(crate::events::RuntimeEvent::SkillRecoveryCompleted {
                                status: crate::skill_recovery::RecoveryStatus::NewSnapshotPublished,
                                generation: candidate.generation(),
                            });
                    return Ok(crate::skill_recovery::SkillRecoveryReport {
                        status: crate::skill_recovery::RecoveryStatus::NewSnapshotPublished,
                        generation: candidate.generation(),
                        quarantined_revisions: quarantined.revisions,
                        maintenance_diagnostics,
                    });
                }
            }
        }

        let SkillManagerMode::Dynamic(config) = &self.inner.mode else {
            unreachable!("managed runtime cannot be bound to a static manager")
        };
        let generation = self.current_snapshot().generation();
        let candidate =
            Arc::new(build_snapshot_with_runtime(config, generation, Some(&backend)).await?);
        backend
            .state
            .persist_initial_active_snapshot(
                candidate.generation(),
                &crate::skill_recovery::snapshot_members(&candidate),
            )
            .await?;
        *self
            .inner
            .current
            .write()
            .expect("skill snapshot lock poisoned") = candidate.clone();
        let _ = backend
            .events
            .send(crate::events::RuntimeEvent::SkillRecoveryCompleted {
                status: crate::skill_recovery::RecoveryStatus::NewSnapshotPublished,
                generation: candidate.generation(),
            });
        Ok(crate::skill_recovery::SkillRecoveryReport {
            status: crate::skill_recovery::RecoveryStatus::NewSnapshotPublished,
            generation: candidate.generation(),
            quarantined_revisions: Vec::new(),
            maintenance_diagnostics,
        })
    }

    async fn rebuild_persisted_snapshot(
        &self,
        backend: &ManagedRuntimeBackend,
        record: &crate::skill_state::SkillSnapshotRecord,
    ) -> anyhow::Result<Arc<SkillSnapshot>> {
        let SkillManagerMode::Dynamic(config) = &self.inner.mode else {
            anyhow::bail!("static skill manager cannot rebuild managed snapshots");
        };
        let members = crate::skill_recovery::parse_snapshot_members(record.members_json.clone())?;
        let mut non_managed = discover_non_managed_packages_read_only(config).await?;
        let managed_source =
            crate::skill_source::ManagedSkillSource::from_store(backend.revisions.clone());
        let mut packages = Vec::with_capacity(members.len());
        for member in &members {
            let package_id = SkillPackageId::parse(&member.package_id)?;
            let package = if member.layer == "managed" {
                let revision_id = member
                    .revision_id
                    .as_deref()
                    .context("managed snapshot member has no revision id")?;
                managed_source
                    .load_managed_revision(&package_id, revision_id)
                    .await?
            } else {
                let layer = match member.layer.as_str() {
                    "builtin" => SkillLayer::Builtin,
                    "session" => SkillLayer::Session,
                    _ => anyhow::bail!("invalid persisted snapshot layer"),
                };
                let index = non_managed
                    .iter()
                    .position(|package| {
                        package.layer == layer && package.descriptor.id == package_id
                    })
                    .context("persisted non-managed snapshot member is unavailable")?;
                non_managed.swap_remove(index)
            };
            if package.descriptor.version.to_string() != member.version
                || package.content_hash != member.content_hash
                || package.descriptor.id != package_id
            {
                anyhow::bail!("persisted snapshot member verification failed");
            }
            packages.push(package);
        }
        let verified =
            build_snapshot_from_packages(config, record.generation, packages.clone()).await?;
        if crate::skill_recovery::snapshot_members(&verified) != record.members_json {
            anyhow::bail!("persisted snapshot resolution does not match its members");
        }
        let snapshot = Arc::new(
            circuit::rebuild_persisted_snapshot_with_circuits(
                config,
                record.generation,
                packages,
                backend,
            )
            .await?,
        );
        if crate::skill_recovery::snapshot_members(&snapshot) != record.members_json {
            anyhow::bail!("persisted snapshot resolution does not match its members");
        }
        Ok(snapshot)
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
        self.begin_publication()
            .await?
            .preview_candidate(candidate)
            .await
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
        let backend = self
            .inner
            .managed_runtime
            .read()
            .expect("managed skill runtime lock poisoned")
            .clone();
        let candidate =
            Arc::new(build_snapshot_with_runtime(config, generation, backend.as_ref()).await?);
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
                managed_runtime: RwLock::new(None),
                live_snapshots: std::sync::Mutex::new(Vec::new()),
            }),
        }
    }
}

fn config_for(manager: &SkillManager) -> anyhow::Result<&SkillManagerConfig> {
    let SkillManagerMode::Dynamic(config) = &manager.inner.mode else {
        anyhow::bail!("static skill manager has no dynamic configuration");
    };
    Ok(config)
}

#[derive(Default)]
struct SnapshotQuarantineResult {
    revisions: Vec<String>,
    failures: usize,
}

impl SkillPublicationGuard {
    pub(crate) fn base_generation(&self) -> u64 {
        self.previous.generation()
    }

    pub(crate) fn base_snapshot(&self) -> Arc<SkillSnapshot> {
        self.previous.clone()
    }

    pub(crate) async fn preview_candidate(
        &self,
        candidate: DiscoveredSkillPackage,
    ) -> anyhow::Result<SkillSnapshotPreview> {
        let SkillManagerMode::Dynamic(config) = &self.manager.inner.mode else {
            anyhow::bail!("static skill manager cannot preview a managed candidate");
        };
        if candidate.layer != SkillLayer::Managed {
            anyhow::bail!("skill preview candidate must use the managed layer");
        }
        let packages = discover_packages_read_only(config).await?;
        let candidate_snapshot = Arc::new(
            build_snapshot_with_candidate(config, self.previous.generation(), packages, candidate)
                .await?,
        );
        Ok(SkillSnapshotPreview {
            base: self.previous.clone(),
            candidate: candidate_snapshot,
        })
    }

    pub(crate) async fn inspect_sources(&self) -> anyhow::Result<SkillPublicationSourceView> {
        let SkillManagerMode::Dynamic(config) = &self.manager.inner.mode else {
            anyhow::bail!("static skill manager cannot inspect publication sources");
        };
        Ok(SkillPublicationSourceView {
            packages: discover_packages_read_only(config).await?,
        })
    }

    pub(crate) async fn build_candidate(
        &self,
        sources: &SkillPublicationSourceView,
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
            build_snapshot_with_candidate(config, generation, sources.packages.clone(), candidate)
                .await?,
        ))
    }

    pub(crate) async fn build_without_managed(
        &self,
        sources: &SkillPublicationSourceView,
        package_id: &SkillPackageId,
    ) -> anyhow::Result<Arc<SkillSnapshot>> {
        let SkillManagerMode::Dynamic(config) = &self.manager.inner.mode else {
            anyhow::bail!("static skill manager cannot publish");
        };
        let generation = self
            .previous
            .generation()
            .checked_add(1)
            .context("skill snapshot generation overflow")?;
        let packages = sources
            .packages
            .iter()
            .filter(|package| {
                package.layer != SkillLayer::Managed || package.descriptor.id != *package_id
            })
            .cloned()
            .collect();
        Ok(Arc::new(
            build_snapshot_from_packages(config, generation, packages).await?,
        ))
    }

    pub(crate) fn publish(&self, candidate: Arc<SkillSnapshot>) -> SkillReloadReport {
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

async fn build_snapshot_with_runtime(
    config: &SkillManagerConfig,
    generation: u64,
    backend: Option<&ManagedRuntimeBackend>,
) -> anyhow::Result<SkillSnapshot> {
    let packages = discover_packages(config).await?;
    let Some(backend) = backend else {
        return build_snapshot_from_packages(config, generation, packages).await;
    };
    circuit::build_snapshot_from_packages_with_circuits(config, generation, packages, backend).await
}

async fn build_snapshot_with_candidate(
    config: &SkillManagerConfig,
    generation: u64,
    mut packages: Vec<DiscoveredSkillPackage>,
    candidate: DiscoveredSkillPackage,
) -> anyhow::Result<SkillSnapshot> {
    let candidate_id = candidate.descriptor.id.clone();
    packages.retain(|package| {
        !(package.layer == SkillLayer::Managed && package.descriptor.id == candidate_id)
    });
    packages.push(candidate);
    build_snapshot_from_packages(config, generation, packages).await
}

impl SkillPublicationSourceView {
    pub(crate) fn has_builtin(&self, package_id: &SkillPackageId) -> bool {
        self.packages.iter().any(|package| {
            package.layer == SkillLayer::Builtin && package.descriptor.id == *package_id
        })
    }

    pub(crate) async fn verify_managed_bindings(&self) -> anyhow::Result<()> {
        for package in &self.packages {
            if package.layer != SkillLayer::Managed {
                continue;
            }
            let binding = package
                .verified_content
                .as_ref()
                .and_then(|content| content.execution_binding.as_ref())
                .context("managed publication source has no execution binding")?;
            binding
                .store
                .verify_managed_binding(
                    &binding.package_id,
                    &binding.revision_id,
                    &binding.storage_path,
                    &package.content_hash,
                )
                .await?;
        }
        Ok(())
    }
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

async fn discover_packages_read_only(
    config: &SkillManagerConfig,
) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
    let mut packages = Vec::new();
    for source in &config.sources {
        let layer = source.layer();
        let discovered = source.discover_read_only().await?;
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

async fn discover_non_managed_packages_read_only(
    config: &SkillManagerConfig,
) -> anyhow::Result<Vec<DiscoveredSkillPackage>> {
    let mut packages = Vec::new();
    for source in &config.sources {
        let layer = source.layer();
        if layer == SkillLayer::Managed {
            continue;
        }
        let discovered = source.discover_read_only().await?;
        if discovered.iter().any(|package| package.layer != layer) {
            anyhow::bail!("skill source returned a package with a mismatched layer");
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
