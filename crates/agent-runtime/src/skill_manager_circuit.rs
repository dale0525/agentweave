use super::*;

impl SkillManager {
    pub async fn lease_snapshot_for_turn(&self) -> anyhow::Result<SkillSnapshotLease> {
        self.converge_to_authoritative_generation().await?;
        self.refresh_expired_circuits().await?;
        self.lease_authoritative_snapshot().await
    }

    pub(crate) async fn converge_to_authoritative_generation(&self) -> anyhow::Result<()> {
        let Ok(backend) = self.managed_runtime() else {
            return Ok(());
        };
        let _guard = self.inner.reload_lock.lock().await;
        self.converge_under_lock(&backend).await.map(|_| ())
    }

    async fn lease_authoritative_snapshot(&self) -> anyhow::Result<SkillSnapshotLease> {
        let Ok(backend) = self.managed_runtime() else {
            return Ok(self.lease_snapshot());
        };
        let _guard = self.inner.reload_lock.lock().await;
        for _ in 0..3 {
            let Some((snapshot, members)) = self.converge_under_lock(&backend).await? else {
                return Ok(self.lease_snapshot());
            };
            let revisions = crate::skill_recovery::snapshot_revision_ids(&snapshot);
            match backend
                .state
                .acquire_snapshot_lease(snapshot.generation(), &members, &revisions)
                .await
            {
                Ok(lease_id) => {
                    self.track_live_snapshot(&snapshot);
                    return Ok(SkillSnapshotLease::new_durable(
                        snapshot,
                        backend.state.clone(),
                        lease_id,
                    ));
                }
                Err(error)
                    if matches!(
                        error.downcast_ref::<crate::skill_state::SkillStateBoundaryError>(),
                        Some(crate::skill_state::SkillStateBoundaryError::Conflict(_))
                    ) => {}
                Err(error) => return Err(error),
            }
        }
        anyhow::bail!("authoritative active snapshot changed repeatedly before turn lease")
    }

    async fn converge_under_lock(
        &self,
        backend: &ManagedRuntimeBackend,
    ) -> anyhow::Result<Option<(Arc<SkillSnapshot>, serde_json::Value)>> {
        let Some(active) = backend
            .state
            .snapshot_with_status(crate::skill_state::SkillSnapshotStatus::Active)
            .await?
        else {
            return Ok(None);
        };
        let current = self.current_snapshot();
        if current.generation() == active.generation
            && crate::skill_recovery::snapshot_members(&current) == active.members_json
        {
            return Ok(Some((current, active.members_json)));
        }
        let converged = self.rebuild_persisted_snapshot(backend, &active).await?;
        *self
            .inner
            .current
            .write()
            .expect("skill snapshot lock poisoned") = converged.clone();
        Ok(Some((converged, active.members_json)))
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
        let publication = self.begin_publication().await?;
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
                    self.publish_circuit_snapshot_with_guard(
                        publication,
                        &package_id,
                        &revision_id,
                        "open_skill_revision_circuit",
                        crate::skill_state_lifecycle::CircuitSnapshotTransition::Open,
                    )
                    .await?;
                }
                crate::skill_state_recovery::CircuitStateTransition::Closed => {
                    self.publish_circuit_snapshot_with_guard(
                        publication,
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
        self.publish_circuit_snapshot_with_guard(
            publication,
            package_id,
            revision_id,
            operation,
            transition,
        )
        .await
    }

    async fn publish_circuit_snapshot_with_guard(
        &self,
        publication: crate::skill_manager::SkillPublicationGuard,
        package_id: &SkillPackageId,
        revision_id: &str,
        operation: &'static str,
        transition: crate::skill_state_lifecycle::CircuitSnapshotTransition,
    ) -> anyhow::Result<()> {
        let backend = self.managed_runtime()?;
        let SkillManagerMode::Dynamic(config) = &self.inner.mode else {
            anyhow::bail!("static skill manager cannot publish circuit state");
        };
        let mut baseline = publication.base_snapshot();
        let mut previous_generation = baseline.generation();
        let mut previous_members = crate::skill_recovery::snapshot_members(&baseline);
        let mut authoritative_record = None;
        let mut conflicts = 0_u8;
        loop {
            let generation = previous_generation
                .checked_add(1)
                .context("skill snapshot generation overflow")?;
            let packages = discover_packages(config).await?;
            let build = build_snapshot_from_packages_with_circuit_observations(
                config, generation, packages, &backend,
            )
            .await?;
            let candidate_members = crate::skill_recovery::snapshot_members(&build.snapshot);
            let mut mutations = circuit_snapshot_mutations(
                &previous_members,
                &build,
                &backend,
                "open_skill_revision_circuit",
                "expire_skill_revision_circuit",
            )
            .await?;
            backend
                .revisions
                .checkpoint(crate::skill_store_faults::StoreFaultPoint::CircuitBeforeDurableCommit)
                .await;
            let selected = mutations.iter_mut().find(|mutation| {
                mutation.package_id == *package_id
                    && mutation.revision_id == revision_id
                    && mutation.transition == transition
            });
            if let Some(selected) = selected {
                selected.operation = operation;
            } else if mutations.is_empty() && candidate_members == previous_members {
                let current = self.current_snapshot();
                if current.generation() != baseline.generation()
                    || crate::skill_recovery::snapshot_members(&current) != previous_members
                {
                    let converged = if let Some(record) = authoritative_record.as_ref() {
                        self.rebuild_persisted_snapshot(&backend, record).await?
                    } else {
                        baseline
                    };
                    publication.publish(converged);
                }
                return Ok(());
            }
            let candidate = Arc::new(build.snapshot);
            let durable = backend
                .state
                .commit_exact_snapshot_publication(
                    crate::skill_state_lifecycle::ExactSnapshotPublication {
                        actor_id: "system-circuit",
                        previous_generation,
                        previous_members: previous_members.clone(),
                        generation,
                        members: candidate_members,
                        circuit_mutations: &mutations,
                    },
                )
                .await;
            if durable.is_ok() {
                backend
                    .revisions
                    .checkpoint(
                        crate::skill_store_faults::StoreFaultPoint::CircuitAfterDurableCommit,
                    )
                    .await;
                let report = publication.publish(candidate);
                let _ = backend
                    .events
                    .send(crate::events::RuntimeEvent::SkillSnapshotPublished {
                        generation: report.active_generation,
                    });
                return Ok(());
            }
            let error = durable.expect_err("circuit publication result checked above");
            if !matches!(
                error.downcast_ref::<crate::skill_state::SkillStateBoundaryError>(),
                Some(crate::skill_state::SkillStateBoundaryError::Conflict(_))
            ) {
                return Err(error);
            }
            conflicts = conflicts.saturating_add(1);
            if conflicts >= 3 {
                return Err(error);
            }
            let authoritative = backend
                .state
                .snapshot_with_status(crate::skill_state::SkillSnapshotStatus::Active)
                .await?
                .context("authoritative active snapshot is missing")?;
            baseline = self
                .verify_persisted_snapshot(&backend, &authoritative)
                .await?;
            previous_generation = authoritative.generation;
            previous_members = authoritative.members_json.clone();
            authoritative_record = Some(authoritative);
        }
    }
}

struct CircuitSnapshotBuild {
    snapshot: SkillSnapshot,
    circuit_rows: std::collections::BTreeMap<String, crate::skill_state::SkillCircuitStateRecord>,
    observed_at: chrono::DateTime<chrono::Utc>,
}

pub(super) struct CircuitRecoveryCandidate {
    pub(super) snapshot: Arc<SkillSnapshot>,
    pub(super) mutations: Vec<crate::skill_state_lifecycle::CircuitSnapshotMutation>,
}

pub(super) async fn circuit_recovery_candidate(
    config: &SkillManagerConfig,
    active: &crate::skill_state::SkillSnapshotRecord,
    backend: &ManagedRuntimeBackend,
) -> anyhow::Result<Option<CircuitRecoveryCandidate>> {
    let persisted = crate::skill_recovery::parse_snapshot_members(active.members_json.clone())?;
    let now = chrono::Utc::now();
    let mut recovery_required = false;
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
            recovery_required = true;
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
            recovery_required = true;
            break;
        }
    }
    if !recovery_required {
        return Ok(None);
    }
    let packages = discover_packages_read_only(config).await?;
    let generation = active
        .generation
        .checked_add(1)
        .context("skill snapshot generation overflow")?;
    let build = build_snapshot_from_packages_with_circuit_observations(
        config, generation, packages, backend,
    )
    .await?;
    let mutations = circuit_snapshot_mutations(
        &active.members_json,
        &build,
        backend,
        "recover_open_skill_revision_circuit",
        "recover_closed_skill_revision_circuit",
    )
    .await?;
    let snapshot = Arc::new(build.snapshot);
    Ok(Some(CircuitRecoveryCandidate {
        snapshot,
        mutations,
    }))
}

async fn circuit_snapshot_mutations(
    previous_members: &serde_json::Value,
    candidate: &CircuitSnapshotBuild,
    backend: &ManagedRuntimeBackend,
    open_operation: &'static str,
    consume_operation: &'static str,
) -> anyhow::Result<Vec<crate::skill_state_lifecycle::CircuitSnapshotMutation>> {
    let persisted = crate::skill_recovery::parse_snapshot_members(previous_members.clone())?;
    let candidate_members = crate::skill_recovery::parse_snapshot_members(
        crate::skill_recovery::snapshot_members(&candidate.snapshot),
    )?;
    let mut mutations = Vec::new();
    for installation in backend.state.list_active_installations().await? {
        if installation.source_layer != crate::skill_state::SkillLayerRecord::Managed {
            continue;
        }
        let Some(revision_id) = installation.active_revision_id.as_deref() else {
            continue;
        };
        let was_visible = persisted.iter().any(|member| {
            member.package_id == installation.package_id.as_str()
                && member.revision_id.as_deref() == Some(revision_id)
        });
        let will_be_visible = candidate_members.iter().any(|member| {
            member.package_id == installation.package_id.as_str()
                && member.revision_id.as_deref() == Some(revision_id)
        });
        let circuit = candidate.circuit_rows.get(revision_id);
        let open = circuit
            .and_then(|state| state.open_until)
            .is_some_and(|deadline| deadline > candidate.observed_at);
        let omission = backend.state.circuit_omission(revision_id).await?;
        let pending_omission = omission.as_ref().is_some_and(|record| {
            !record.consumed
                && record.package_id == installation.package_id
                && record.revision_id == revision_id
        });
        let transition = if open && was_visible && !will_be_visible {
            Some((
                crate::skill_state_lifecycle::CircuitSnapshotTransition::Open,
                open_operation,
            ))
        } else if !open && pending_omission {
            Some((
                crate::skill_state_lifecycle::CircuitSnapshotTransition::Consume,
                consume_operation,
            ))
        } else {
            None
        };
        if let Some((transition, operation)) = transition {
            let expected_circuit = circuit
                .cloned()
                .context("circuit mutation state is missing")?;
            mutations.push(crate::skill_state_lifecycle::CircuitSnapshotMutation {
                package_id: installation.package_id,
                revision_id: revision_id.to_string(),
                expected_circuit,
                transition,
                operation,
            });
        }
    }
    mutations.sort_by(|left, right| left.revision_id.cmp(&right.revision_id));
    Ok(mutations)
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
    Ok(build_snapshot_from_packages_with_circuit_observations(
        config, generation, packages, backend,
    )
    .await?
    .snapshot)
}

async fn build_snapshot_from_packages_with_circuit_observations(
    config: &SkillManagerConfig,
    generation: u64,
    packages: Vec<DiscoveredSkillPackage>,
    backend: &ManagedRuntimeBackend,
) -> anyhow::Result<CircuitSnapshotBuild> {
    let observed_at = chrono::Utc::now();
    let mut circuit_rows = std::collections::BTreeMap::new();
    for package in &packages {
        let revision_id = package
            .verified_content
            .as_ref()
            .and_then(|content| content.execution_binding.as_ref())
            .map(|binding| binding.revision_id.as_str());
        if let Some(revision_id) = revision_id
            && let Some(row) = backend.state.get_circuit_state(revision_id).await?
        {
            circuit_rows.insert(revision_id.to_string(), row);
        }
    }
    let mut eligible = Vec::with_capacity(packages.len());
    let mut circuit_open = Vec::new();
    for package in packages {
        let revision_id = package
            .verified_content
            .as_ref()
            .and_then(|content| content.execution_binding.as_ref())
            .map(|binding| binding.revision_id.as_str());
        if let Some(revision_id) = revision_id
            && circuit_rows
                .get(revision_id)
                .and_then(|state| state.open_until)
                .is_some_and(|deadline| deadline > observed_at)
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
    let snapshot = SkillSnapshot::build(generation, resolved)
        .await
        .map(|snapshot| {
            snapshot.with_platform_capabilities(config.platform, config.capabilities.clone())
        })?;
    Ok(CircuitSnapshotBuild {
        snapshot,
        circuit_rows,
        observed_at,
    })
}
