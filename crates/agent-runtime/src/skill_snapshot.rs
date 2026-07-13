use crate::platform::{CapabilitySet, PlatformId};
use crate::skill::SkillRegistry;
use crate::skill_catalog::{SkillCatalog, SkillCatalogEntry};
use crate::skill_resolver::{ResolvedSkillPackage, ResolvedSkillSet};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub struct SkillSnapshot {
    generation: u64,
    packages: Vec<ResolvedSkillPackage>,
    inactive: Vec<ResolvedSkillPackage>,
    registry: SkillRegistry,
    catalog: SkillCatalog,
}

#[derive(Clone)]
pub struct SkillSnapshotLease {
    snapshot: Arc<SkillSnapshot>,
    durable: Option<Arc<DurableSnapshotLease>>,
}

impl SkillSnapshotLease {
    pub(crate) fn new(snapshot: Arc<SkillSnapshot>) -> Self {
        Self {
            snapshot,
            durable: None,
        }
    }

    pub(crate) fn new_durable(
        snapshot: Arc<SkillSnapshot>,
        state: crate::skill_state::SkillStateStore,
        lease_id: String,
    ) -> Self {
        let cancellation = tokio_util::sync::CancellationToken::new();
        let heartbeat_cancel = cancellation.clone();
        let heartbeat_state = state.clone();
        let heartbeat_id = lease_id.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = heartbeat_cancel.cancelled() => return,
                    _ = tokio::time::sleep(crate::skill_state_leases::SNAPSHOT_LEASE_HEARTBEAT) => {
                        match heartbeat_state.refresh_snapshot_lease(&heartbeat_id).await {
                            Ok(true) => {}
                            Ok(false) => return,
                            Err(error) => tracing::warn!(
                                lease_id = %heartbeat_id,
                                ?error,
                                "durable snapshot lease heartbeat will retry after database error"
                            ),
                        }
                    }
                }
            }
        });
        Self {
            snapshot,
            durable: Some(Arc::new(DurableSnapshotLease {
                state,
                lease_id,
                cancellation,
            })),
        }
    }

    pub fn snapshot(&self) -> &SkillSnapshot {
        &self.snapshot
    }

    pub fn snapshot_arc(&self) -> Arc<SkillSnapshot> {
        self.snapshot.clone()
    }

    pub fn generation(&self) -> u64 {
        self.snapshot.generation()
    }
}

impl std::fmt::Debug for SkillSnapshotLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SkillSnapshotLease")
            .field("generation", &self.generation())
            .field("durable", &self.durable.is_some())
            .finish()
    }
}

struct DurableSnapshotLease {
    state: crate::skill_state::SkillStateStore,
    lease_id: String,
    cancellation: tokio_util::sync::CancellationToken,
}

impl Drop for DurableSnapshotLease {
    fn drop(&mut self) {
        self.cancellation.cancel();
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let state = self.state.clone();
        let lease_id = self.lease_id.clone();
        runtime.spawn(async move {
            let _ = state.release_snapshot_lease(&lease_id).await;
        });
    }
}

impl SkillSnapshot {
    pub async fn build(generation: u64, resolved: ResolvedSkillSet) -> anyhow::Result<Self> {
        let registry = build_registry(&resolved.active).await?;
        let catalog = build_catalog(&resolved.active).await?;
        Ok(Self {
            generation,
            packages: resolved.active,
            inactive: resolved.inactive,
            registry,
            catalog,
        })
    }

    pub fn from_registry_and_catalog(
        generation: u64,
        registry: SkillRegistry,
        catalog: SkillCatalog,
    ) -> Self {
        Self {
            generation,
            packages: Vec::new(),
            inactive: Vec::new(),
            registry,
            catalog,
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn packages(&self) -> &[ResolvedSkillPackage] {
        &self.packages
    }

    pub fn inactive(&self) -> &[ResolvedSkillPackage] {
        &self.inactive
    }

    pub fn registry(&self) -> &SkillRegistry {
        &self.registry
    }

    pub fn catalog(&self) -> &SkillCatalog {
        &self.catalog
    }

    pub(crate) fn with_platform_capabilities(
        mut self,
        platform: PlatformId,
        capabilities: CapabilitySet,
    ) -> Self {
        self.registry = self
            .registry
            .with_platform_capabilities(platform, capabilities);
        self
    }
}

async fn build_registry(packages: &[ResolvedSkillPackage]) -> anyhow::Result<SkillRegistry> {
    let mut skills = Vec::new();
    for resolved in packages {
        if resolved.package.descriptor.package.include_runtime {
            match &resolved.package.verified_content {
                Some(verified) => {
                    let bytes = verified.runtime_manifest.as_deref().ok_or_else(|| {
                        anyhow::anyhow!("managed runtime package has no verified skill.json bytes")
                    })?;
                    skills.push(
                        SkillRegistry::load_verified_skill(
                            resolved.package.root.clone(),
                            bytes,
                            &resolved.package.descriptor.id,
                            verified.expected_content_hash.clone(),
                            verified.limits.package_limits(),
                            verified
                                .execution_binding
                                .clone()
                                .map(Box::new)
                                .map(crate::skill_verified::VerifiedExecutionBinding::Managed)
                                .or_else(|| {
                                    verified.bundle_execution_binding.clone().map(Box::new).map(
                                        crate::skill_verified::VerifiedExecutionBinding::Bundle,
                                    )
                                }),
                        )
                        .await?,
                    );
                }
                None => skills.push(
                    SkillRegistry::load_development_skill_for_package(
                        &resolved.package.root,
                        &resolved.package.descriptor.id,
                    )
                    .await?,
                ),
            }
        }
    }
    SkillRegistry::from_installed(skills)
}

async fn build_catalog(packages: &[ResolvedSkillPackage]) -> anyhow::Result<SkillCatalog> {
    let mut entries = Vec::<SkillCatalogEntry>::new();
    for resolved in packages {
        if resolved.package.descriptor.package.include_instructions {
            match &resolved.package.verified_content {
                Some(verified) => {
                    let bytes = verified.instructions_file.as_deref().ok_or_else(|| {
                        anyhow::anyhow!(
                            "managed instruction package has no verified SKILL.md bytes"
                        )
                    })?;
                    entries.push(SkillCatalog::read_verified_package_entry(
                        std::path::PathBuf::from("SKILL.md"),
                        bytes,
                    )?);
                }
                None => {
                    entries.push(SkillCatalog::read_package_entry(&resolved.package.root).await?)
                }
            }
        }
    }
    SkillCatalog::from_entries(entries)
}
