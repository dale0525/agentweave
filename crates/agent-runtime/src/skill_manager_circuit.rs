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
                    self.publish_circuit_snapshot_owned(
                        &package_id,
                        &revision_id,
                        "open_skill_revision_circuit",
                        crate::skill_state_lifecycle::CircuitSnapshotTransition::Open,
                    )
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
            self.publish_circuit_snapshot_owned(
                &package_id,
                &revision_id,
                "expire_skill_revision_circuit",
                crate::skill_state_lifecycle::CircuitSnapshotTransition::Consume,
            )
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
        let manager = self.clone();
        let revision_id = revision_id.clone();
        tokio::spawn(async move {
            manager
                .record_managed_execution_result_owned(package_id, revision_id, success)
                .await
        })
        .await
        .map_err(|_| anyhow::anyhow!("owned circuit transition task failed"))?
    }

    async fn record_managed_execution_result_owned(
        &self,
        package_id: SkillPackageId,
        revision_id: String,
        success: bool,
    ) -> anyhow::Result<()> {
        let backend = self.managed_runtime()?;
        let update = backend
            .state
            .record_managed_execution_result(&package_id, &revision_id, success, chrono::Utc::now())
            .await?;
        if let Some((_, transition)) = update {
            match transition {
                crate::skill_state_recovery::CircuitStateTransition::Opened => {
                    backend
                        .revisions
                        .checkpoint(
                            crate::skill_store_faults::StoreFaultPoint::CircuitAfterStateTransition,
                        )
                        .await;
                    self.publish_circuit_snapshot(
                        &package_id,
                        &revision_id,
                        "open_skill_revision_circuit",
                        crate::skill_state_lifecycle::CircuitSnapshotTransition::Open,
                    )
                    .await?;
                }
                crate::skill_state_recovery::CircuitStateTransition::Closed => {
                    self.publish_circuit_snapshot(
                        &package_id,
                        &revision_id,
                        "close_skill_revision_circuit",
                        crate::skill_state_lifecycle::CircuitSnapshotTransition::Consume,
                    )
                    .await?;
                }
                crate::skill_state_recovery::CircuitStateTransition::None => {}
            }
        }
        Ok(())
    }

    async fn publish_circuit_snapshot_owned(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        operation: &'static str,
        transition: crate::skill_state_lifecycle::CircuitSnapshotTransition,
    ) -> anyhow::Result<()> {
        let manager = self.clone();
        let package_id = package_id.clone();
        let revision_id = revision_id.to_string();
        tokio::spawn(async move {
            manager
                .publish_circuit_snapshot(&package_id, &revision_id, operation, transition)
                .await
        })
        .await
        .map_err(|_| anyhow::anyhow!("owned circuit publication task failed"))?
    }

    async fn publish_circuit_snapshot(
        &self,
        package_id: &SkillPackageId,
        revision_id: &str,
        operation: &'static str,
        transition: crate::skill_state_lifecycle::CircuitSnapshotTransition,
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
        let durable = backend
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
                    revision_id,
                    circuit_transition: transition,
                },
            )
            .await;
        if let Err(error) = durable {
            if !matches!(
                error.downcast_ref::<crate::skill_state::SkillStateBoundaryError>(),
                Some(crate::skill_state::SkillStateBoundaryError::Conflict(_))
            ) {
                return Err(error);
            }
            let authoritative = backend
                .state
                .snapshot_with_status(crate::skill_state::SkillSnapshotStatus::Active)
                .await?
                .context("authoritative active snapshot is missing")?;
            if authoritative.generation <= publication.base_generation() {
                return Err(error);
            }
            let snapshot = self
                .rebuild_persisted_snapshot(&backend, &authoritative)
                .await?;
            publication.publish(snapshot);
            return Ok(());
        }
        backend
            .revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::CircuitAfterDurableCommit)
            .await;
        let report = publication.publish(candidate);
        let _ = backend
            .events
            .send(crate::events::RuntimeEvent::SkillSnapshotPublished {
                generation: report.active_generation,
            });
        Ok(())
    }
}

pub(super) struct CircuitRecoveryCandidate {
    pub(super) snapshot: Arc<SkillSnapshot>,
    pub(super) package_id: SkillPackageId,
    pub(super) revision_id: String,
    pub(super) transition: crate::skill_state_lifecycle::CircuitSnapshotTransition,
}

pub(super) async fn circuit_recovery_candidate(
    config: &SkillManagerConfig,
    active: &crate::skill_state::SkillSnapshotRecord,
    backend: &ManagedRuntimeBackend,
) -> anyhow::Result<Option<CircuitRecoveryCandidate>> {
    let persisted = crate::skill_recovery::parse_snapshot_members(active.members_json.clone())?;
    let now = chrono::Utc::now();
    let mut recovery = None;
    for installation in backend.state.list_active_installations().await? {
        if installation.source_layer != crate::skill_state::SkillLayerRecord::Managed {
            continue;
        }
        let Some(revision_id) = installation.active_revision_id.as_deref() else {
            continue;
        };
        let persisted_member = persisted.iter().any(|member| {
            member.package_id == installation.package_id.as_str()
                && member.revision_id.as_deref() == Some(revision_id)
        });
        let open = backend.state.circuit_is_open(revision_id, now).await?;
        let omission = backend.state.circuit_omission(revision_id).await?;
        if persisted_member && open {
            recovery = Some((
                installation.package_id,
                revision_id.to_string(),
                crate::skill_state_lifecycle::CircuitSnapshotTransition::Open,
            ));
            break;
        }
        if !persisted_member
            && !open
            && omission.as_ref().is_some_and(|record| {
                !record.consumed
                    && record.package_id == installation.package_id
                    && record.revision_id == revision_id
            })
        {
            recovery = Some((
                installation.package_id,
                revision_id.to_string(),
                crate::skill_state_lifecycle::CircuitSnapshotTransition::Consume,
            ));
            break;
        }
    }
    let Some((package_id, revision_id, transition)) = recovery else {
        return Ok(None);
    };
    let packages = discover_packages_read_only(config).await?;
    let generation = active
        .generation
        .checked_add(1)
        .context("skill snapshot generation overflow")?;
    Ok(Some(CircuitRecoveryCandidate {
        snapshot: Arc::new(
            build_snapshot_from_packages_with_circuits(config, generation, packages, backend)
                .await?,
        ),
        package_id,
        revision_id,
        transition,
    }))
}

pub(super) async fn rebuild_persisted_snapshot_with_circuits(
    config: &SkillManagerConfig,
    generation: u64,
    mut packages: Vec<DiscoveredSkillPackage>,
    backend: &ManagedRuntimeBackend,
) -> anyhow::Result<SkillSnapshot> {
    let managed = crate::skill_source::ManagedSkillSource::from_store(backend.revisions.clone())
        .discover_valid_active_read_only()
        .await?;
    for package in managed {
        if packages
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
            && backend.state.circuit_omission(revision_id).await?.is_some()
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
