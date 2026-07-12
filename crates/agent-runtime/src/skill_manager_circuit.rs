use super::*;

impl SkillManager {
    pub async fn lease_snapshot_for_turn(&self) -> anyhow::Result<SkillSnapshotLease> {
        self.refresh_expired_circuits().await?;
        Ok(self.lease_snapshot())
    }

    async fn refresh_expired_circuits(&self) -> anyhow::Result<()> {
        let current = self.current_snapshot();
        let active_managed = current.packages().iter().filter_map(|resolved| {
            resolved
                .package
                .verified_content
                .as_ref()
                .and_then(|content| content.execution_binding.as_ref())
                .map(|binding| {
                    (
                        resolved.package.descriptor.id.clone(),
                        binding.revision_id.clone(),
                    )
                })
        });
        let backend = self.managed_runtime().ok();
        if let Some(backend) = &backend {
            for (package_id, revision_id) in active_managed {
                if backend
                    .state
                    .circuit_is_open(&revision_id, chrono::Utc::now())
                    .await?
                {
                    self.publish_circuit_snapshot(&package_id, "open_skill_revision_circuit")
                        .await?;
                    return Ok(());
                }
            }
        }
        let expired = current.inactive().iter().find_map(|resolved| {
            (resolved.status == crate::skill_resolver::SkillResolutionStatus::CircuitOpen)
                .then(|| {
                    resolved
                        .package
                        .verified_content
                        .as_ref()
                        .and_then(|content| content.execution_binding.as_ref())
                        .map(|binding| {
                            (
                                resolved.package.descriptor.id.clone(),
                                binding.revision_id.clone(),
                            )
                        })
                })
                .flatten()
        });
        let Some((package_id, revision_id)) = expired else {
            return Ok(());
        };
        let backend =
            backend.ok_or_else(|| anyhow::anyhow!("managed skill runtime is not bound"))?;
        if !backend
            .state
            .circuit_is_open(&revision_id, chrono::Utc::now())
            .await?
        {
            self.publish_circuit_snapshot(&package_id, "expire_skill_revision_circuit")
                .await?;
        }
        Ok(())
    }

    pub async fn record_execution_result(
        &self,
        source: &crate::tools::ToolSource,
        success: bool,
    ) -> anyhow::Result<()> {
        let crate::tools::ToolSource::RuntimeSkill {
            package_id,
            revision_id: Some(revision_id),
            ..
        } = source
        else {
            return Ok(());
        };
        let Ok(package_id) = SkillPackageId::parse(package_id) else {
            return Ok(());
        };
        let backend = self.managed_runtime()?;
        let update = backend
            .state
            .record_managed_execution_result(&package_id, revision_id, success, chrono::Utc::now())
            .await?;
        if update.is_some_and(|(_, opened)| opened) {
            self.publish_circuit_snapshot(&package_id, "open_skill_revision_circuit")
                .await?;
        }
        Ok(())
    }

    async fn publish_circuit_snapshot(
        &self,
        package_id: &SkillPackageId,
        operation: &'static str,
    ) -> anyhow::Result<()> {
        let publication = self.begin_publication().await?;
        let backend = self.managed_runtime()?;
        let SkillManagerMode::Dynamic(config) = &self.inner.mode else {
            anyhow::bail!("static skill manager cannot publish circuit state");
        };
        let generation = publication
            .base_generation()
            .checked_add(1)
            .context("skill snapshot generation overflow")?;
        let candidate =
            Arc::new(build_snapshot_with_runtime(config, generation, Some(&backend)).await?);
        backend
            .state
            .commit_exact_snapshot_publication(
                crate::skill_state_lifecycle::ExactSnapshotPublication {
                    actor_id: "system-circuit",
                    operation,
                    package_id,
                    previous_generation: publication.base_generation(),
                    previous_members: crate::skill_recovery::snapshot_members(
                        &publication.base_snapshot(),
                    ),
                    generation,
                    members: crate::skill_recovery::snapshot_members(&candidate),
                },
            )
            .await?;
        let report = publication.publish(candidate);
        let _ = backend
            .events
            .send(crate::events::RuntimeEvent::SkillSnapshotPublished {
                generation: report.active_generation,
            });
        Ok(())
    }
}

pub(super) async fn rebuild_persisted_snapshot_with_circuits(
    config: &SkillManagerConfig,
    generation: u64,
    mut packages: Vec<DiscoveredSkillPackage>,
    backend: &ManagedRuntimeBackend,
) -> anyhow::Result<SkillSnapshot> {
    for package in discover_packages(config).await? {
        if package.layer != SkillLayer::Managed
            || packages
                .iter()
                .any(|persisted| persisted.descriptor.id == package.descriptor.id)
        {
            continue;
        }
        let open = package
            .verified_content
            .as_ref()
            .and_then(|content| content.execution_binding.as_ref())
            .map(|binding| binding.revision_id.as_str());
        if let Some(revision_id) = open
            && backend
                .state
                .circuit_is_open(revision_id, chrono::Utc::now())
                .await?
        {
            packages.push(package);
        }
    }
    build_snapshot_from_packages_with_circuits(config, generation, packages, backend).await
}

pub(super) async fn build_snapshot_from_packages_with_circuits(
    config: &SkillManagerConfig,
    generation: u64,
    packages: Vec<DiscoveredSkillPackage>,
    backend: &ManagedRuntimeBackend,
) -> anyhow::Result<SkillSnapshot> {
    let mut eligible = Vec::with_capacity(packages.len());
    let mut circuit_open = Vec::new();
    for package in packages {
        let revision_id = package
            .verified_content
            .as_ref()
            .and_then(|content| content.execution_binding.as_ref())
            .map(|binding| binding.revision_id.as_str());
        if let Some(revision_id) = revision_id
            && backend
                .state
                .circuit_is_open(revision_id, chrono::Utc::now())
                .await?
        {
            circuit_open.push(package);
        } else {
            eligible.push(package);
        }
    }
    let mut resolved = SkillResolver::resolve(SkillResolutionInput {
        packages: eligible,
        platform: config.platform,
        capabilities: config.capabilities.clone(),
        protected_packages: config.protected_packages.clone(),
        allowed_overrides: config.allowed_overrides.clone(),
        runtime_version: config.runtime_version.clone(),
    })?;
    resolved
        .inactive
        .extend(circuit_open.into_iter().map(|package| {
            crate::skill_resolver::ResolvedSkillPackage {
                package,
                status: crate::skill_resolver::SkillResolutionStatus::CircuitOpen,
                reason: "managed revision circuit open".into(),
            }
        }));
    SkillSnapshot::build(generation, resolved)
        .await
        .map(|snapshot| {
            snapshot.with_platform_capabilities(config.platform, config.capabilities.clone())
        })
}
