use super::*;

impl SkillManager {
    pub(super) async fn rebuild_persisted_snapshot(
        &self,
        backend: &ManagedRuntimeBackend,
        record: &crate::skill_state::SkillSnapshotRecord,
    ) -> anyhow::Result<Arc<SkillSnapshot>> {
        let (config, packages, _) = self.persisted_snapshot_parts(backend, record).await?;
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

    pub(super) async fn verify_persisted_snapshot(
        &self,
        backend: &ManagedRuntimeBackend,
        record: &crate::skill_state::SkillSnapshotRecord,
    ) -> anyhow::Result<Arc<SkillSnapshot>> {
        let (_, _, verified) = self.persisted_snapshot_parts(backend, record).await?;
        Ok(verified)
    }

    async fn persisted_snapshot_parts<'a>(
        &'a self,
        backend: &ManagedRuntimeBackend,
        record: &crate::skill_state::SkillSnapshotRecord,
    ) -> anyhow::Result<(
        &'a SkillManagerConfig,
        Vec<DiscoveredSkillPackage>,
        Arc<SkillSnapshot>,
    )> {
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
        let verified = Arc::new(
            build_snapshot_from_packages(config, record.generation, packages.clone()).await?,
        );
        if crate::skill_recovery::snapshot_members(&verified) != record.members_json {
            anyhow::bail!("persisted snapshot resolution does not match its members");
        }
        Ok((config, packages, verified))
    }
}

pub(super) async fn quarantine_invalid_snapshot_members(
    backend: &ManagedRuntimeBackend,
    record: &crate::skill_state::SkillSnapshotRecord,
) -> SnapshotQuarantineResult {
    let Ok(members) = crate::skill_recovery::parse_snapshot_members(record.members_json.clone())
    else {
        return SnapshotQuarantineResult {
            revisions: Vec::new(),
            failures: 1,
        };
    };
    let mut result = SnapshotQuarantineResult::default();
    for member in members
        .into_iter()
        .filter(|member| member.layer == "managed")
    {
        let (Ok(package_id), Some(revision_id)) = (
            SkillPackageId::parse(&member.package_id),
            member.revision_id.as_deref(),
        ) else {
            result.failures += 1;
            continue;
        };
        let revision_record = match backend.state.get_revision(revision_id).await {
            Ok(record) => record,
            Err(_) => {
                result.failures += 1;
                continue;
            }
        };
        let descriptor_matches = revision_record
            .as_ref()
            .and_then(|record| {
                serde_json::from_value::<crate::skill_package::SkillPackageDescriptor>(
                    record.descriptor_json.clone(),
                )
                .ok()
                .map(|descriptor| {
                    descriptor.id == package_id && descriptor.version.to_string() == member.version
                })
            })
            .unwrap_or(false);
        let row_matches = revision_record.as_ref().is_some_and(|record| {
            record.package_id == package_id
                && record.revision_id == revision_id
                && record.version == member.version
                && record.content_hash == member.content_hash
                && record.status == crate::skill_state::SkillRevisionStatus::Managed
                && descriptor_matches
        });
        let identity_matches = if row_matches {
            backend
                .revisions
                .proves_managed_revision_identity(
                    revision_record
                        .as_ref()
                        .expect("matching revision checked above"),
                )
                .await
                .unwrap_or(false)
        } else {
            false
        };
        if !identity_matches {
            let key = format!(
                "snapshot-member:{}:{}:{}",
                record.generation, member.package_id, revision_id
            );
            if backend
                .state
                .record_maintenance_diagnostic_once(
                    &key,
                    Some(revision_id),
                    "managed",
                    "snapshot_member_ownership_mismatch",
                    serde_json::json!({
                        "generation": record.generation,
                        "ownership": "unproven"
                    }),
                )
                .await
                .is_err()
            {
                result.failures += 1;
            }
            continue;
        }
        let prepared = match backend
            .revisions
            .prepare_invalid_managed_revision(
                revision_record
                    .as_ref()
                    .expect("matching revision checked above"),
            )
            .await
        {
            Ok(None) => continue,
            Ok(Some(prepared)) => prepared,
            Err(_) => {
                record_changed_before_quarantine(backend, record.generation, &member, revision_id)
                    .await;
                result.failures += 1;
                continue;
            }
        };
        backend
            .revisions
            .checkpoint(crate::skill_store_faults::StoreFaultPoint::RecoveryBeforeQuarantine)
            .await;
        match backend
            .revisions
            .quarantine_prepared_invalid_managed_revision(
                prepared,
                "startup active snapshot verification failed",
            )
            .await
        {
            Ok(_) => result.revisions.push(revision_id.to_string()),
            Err(_) => {
                record_changed_before_quarantine(backend, record.generation, &member, revision_id)
                    .await;
                result.failures += 1;
            }
        }
    }
    result.revisions.sort();
    result
}

async fn record_changed_before_quarantine(
    backend: &ManagedRuntimeBackend,
    generation: u64,
    member: &crate::skill_recovery::PersistedSnapshotMember,
    revision_id: &str,
) {
    let key = format!(
        "snapshot-member-changed:{generation}:{}:{revision_id}",
        member.package_id
    );
    let _ = backend
        .state
        .record_maintenance_diagnostic_once(
            &key,
            Some(revision_id),
            "managed",
            "snapshot_member_changed_before_quarantine",
            serde_json::json!({
                "generation": generation,
                "ownership": "changed"
            }),
        )
        .await;
}
