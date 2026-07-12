use super::*;
use crate::skill_resolver::{ResolvedSkillPackage, ResolvedSkillSet, SkillResolutionStatus};
use crate::skill_state_upgrade::{
    ApplicationGraphState, ApplicationUpdatePublication, ApplicationUpdateTransition,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

pub(super) async fn reconcile_application_update(
    manager: &SkillManager,
    backend: &ManagedRuntimeBackend,
    active: &crate::skill_state::SkillSnapshotRecord,
    active_verified: bool,
) -> anyhow::Result<Option<crate::skill_recovery::SkillRecoveryReport>> {
    if !persisted_managed_members_are_valid(backend, active).await? {
        return Ok(None);
    }
    let config = config_for(manager)?;
    let packages = discover_packages_read_only(config).await?;
    let fingerprint = application_graph_fingerprint(config, &packages)?;
    let graph_state = backend.state.application_graph_state().await?;
    if graph_state.is_none() {
        if active_verified {
            backend
                .state
                .record_initial_application_graph(active.generation, &fingerprint)
                .await?;
        }
        return Ok(None);
    }
    let installations = backend.state.list_active_installations().await?;
    let forced = forced_inactive_packages(config, &packages, active, &installations);
    let graph_changed = graph_state
        .as_ref()
        .is_none_or(|state| state.fingerprint != fingerprint);
    if active_verified && !graph_changed && forced.is_empty() {
        return Ok(None);
    }

    let generation = active
        .generation
        .checked_add(1)
        .context("skill snapshot generation overflow")?;
    let candidate =
        Arc::new(build_application_snapshot(config, generation, packages, backend, &forced).await?);
    let transitions = application_update_transitions(&candidate, installations)?;
    backend
        .state
        .commit_application_update(ApplicationUpdatePublication {
            expected_snapshot: active,
            expected_graph: graph_state.as_ref(),
            fingerprint: &fingerprint,
            generation,
            members: &crate::skill_recovery::snapshot_members(&candidate),
            transitions: &transitions,
        })
        .await?;
    *manager
        .inner
        .current
        .write()
        .expect("skill snapshot lock poisoned") = candidate.clone();
    let _ = backend
        .events
        .send(crate::events::RuntimeEvent::SkillRecoveryCompleted {
            status: crate::skill_recovery::RecoveryStatus::NewSnapshotPublished,
            generation,
        });
    Ok(Some(crate::skill_recovery::SkillRecoveryReport {
        status: crate::skill_recovery::RecoveryStatus::NewSnapshotPublished,
        generation,
        quarantined_revisions: Vec::new(),
        maintenance_diagnostics: 0,
    }))
}

pub(super) async fn has_application_update_authority(
    manager: &SkillManager,
    backend: &ManagedRuntimeBackend,
    active: &crate::skill_state::SkillSnapshotRecord,
) -> anyhow::Result<bool> {
    let Some(graph_state) = backend.state.application_graph_state().await? else {
        return Ok(false);
    };
    if !persisted_managed_members_are_valid(backend, active).await? {
        return Ok(false);
    }
    let config = config_for(manager)?;
    let packages = discover_packages_read_only(config).await?;
    let fingerprint = application_graph_fingerprint(config, &packages)?;
    Ok(graph_state.fingerprint != fingerprint)
}

pub(super) async fn record_initial_application_graph(
    manager: &SkillManager,
    backend: &ManagedRuntimeBackend,
    generation: u64,
) -> anyhow::Result<()> {
    if backend.state.application_graph_state().await?.is_some() {
        return Ok(());
    }
    let config = config_for(manager)?;
    let packages = discover_non_managed_packages_read_only(config).await?;
    let fingerprint = application_graph_fingerprint(config, &packages)?;
    backend
        .state
        .record_initial_application_graph(generation, &fingerprint)
        .await
}

async fn persisted_managed_members_are_valid(
    backend: &ManagedRuntimeBackend,
    active: &crate::skill_state::SkillSnapshotRecord,
) -> anyhow::Result<bool> {
    let members = crate::skill_recovery::parse_snapshot_members(active.members_json.clone())?;
    let source = crate::skill_source::ManagedSkillSource::from_store(backend.revisions.clone());
    for member in members.iter().filter(|member| member.layer == "managed") {
        let package_id = SkillPackageId::parse(&member.package_id)?;
        let Some(revision_id) = member.revision_id.as_deref() else {
            return Ok(false);
        };
        let Ok(package) = source.load_managed_revision(&package_id, revision_id).await else {
            return Ok(false);
        };
        if package.descriptor.version.to_string() != member.version
            || package.content_hash != member.content_hash
        {
            return Ok(false);
        }
    }
    Ok(true)
}

fn application_graph_fingerprint(
    config: &SkillManagerConfig,
    packages: &[DiscoveredSkillPackage],
) -> anyhow::Result<String> {
    let mut graph = packages
        .iter()
        .filter(|package| package.layer != SkillLayer::Managed)
        .map(|package| {
            serde_json::json!({
                "id": package.descriptor.id.as_str(),
                "layer": format!("{:?}", package.layer).to_ascii_lowercase(),
                "version": package.descriptor.version.to_string(),
                "contentHash": package.content_hash
            })
        })
        .collect::<Vec<_>>();
    graph.sort_by_key(|value| value.to_string());
    let mut protected = config
        .protected_packages
        .iter()
        .map(|id| id.as_str())
        .collect::<Vec<_>>();
    protected.sort_unstable();
    let mut overrides = config
        .allowed_overrides
        .iter()
        .map(|id| id.as_str())
        .collect::<Vec<_>>();
    overrides.sort_unstable();
    let document = serde_json::json!({
        "packages": graph,
        "platform": format!("{:?}", config.platform).to_ascii_lowercase(),
        "capabilities": config.capabilities.names(),
        "protectedPackages": protected,
        "allowedOverrides": overrides,
        "runtimeVersion": config.runtime_version.to_string()
    });
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&document)?)))
}

fn forced_inactive_packages(
    config: &SkillManagerConfig,
    packages: &[DiscoveredSkillPackage],
    active: &crate::skill_state::SkillSnapshotRecord,
    installations: &[crate::skill_state::SkillInstallationRecord],
) -> BTreeMap<SkillPackageId, (SkillResolutionStatus, String)> {
    let previously_managed =
        crate::skill_recovery::parse_snapshot_members(active.members_json.clone())
            .unwrap_or_default()
            .into_iter()
            .filter(|member| member.layer == "managed")
            .filter_map(|member| SkillPackageId::parse(&member.package_id).ok())
            .collect::<BTreeSet<_>>();
    let protected = config
        .protected_packages
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>();
    let mut forced = BTreeMap::new();
    for id in previously_managed.intersection(&protected) {
        forced.insert(
            id.clone(),
            (
                SkillResolutionStatus::ProtectedPackage,
                "managed package became inactive because the built-in package is now protected"
                    .into(),
            ),
        );
    }
    for installation in installations {
        if installation.source_layer != crate::skill_state::SkillLayerRecord::Managed
            || installation.trust_level == "approved"
        {
            continue;
        }
        let requests_network = packages.iter().any(|package| {
            package.layer == SkillLayer::Managed
                && package.descriptor.id == installation.package_id
                && package
                    .descriptor
                    .requires
                    .capabilities
                    .iter()
                    .any(|capability| capability.starts_with("network."))
        });
        if requests_network {
            forced.insert(
                installation.package_id.clone(),
                (
                    SkillResolutionStatus::NetworkPolicyUnavailable,
                    "untrusted managed package requires an enforceable network policy".into(),
                ),
            );
        }
    }
    forced
}

async fn build_application_snapshot(
    config: &SkillManagerConfig,
    generation: u64,
    mut packages: Vec<DiscoveredSkillPackage>,
    backend: &ManagedRuntimeBackend,
    forced: &BTreeMap<SkillPackageId, (SkillResolutionStatus, String)>,
) -> anyhow::Result<SkillSnapshot> {
    let mut forced_packages = Vec::new();
    packages.retain(|package| {
        if package.layer == SkillLayer::Managed
            && let Some((status, reason)) = forced.get(&package.descriptor.id)
        {
            forced_packages.push(ResolvedSkillPackage {
                package: package.clone(),
                status: *status,
                reason: reason.clone(),
            });
            false
        } else {
            true
        }
    });
    let base =
        circuit::build_snapshot_from_packages_with_circuits(config, generation, packages, backend)
            .await?;
    if forced_packages.is_empty() {
        return Ok(base);
    }
    let mut resolved = ResolvedSkillSet {
        active: base.packages().to_vec(),
        inactive: base.inactive().to_vec(),
    };
    resolved.inactive.extend(forced_packages);
    SkillSnapshot::build(generation, resolved)
        .await
        .map(|snapshot| {
            snapshot.with_platform_capabilities(config.platform, config.capabilities.clone())
        })
}

fn application_update_transitions(
    candidate: &SkillSnapshot,
    installations: Vec<crate::skill_state::SkillInstallationRecord>,
) -> anyhow::Result<Vec<ApplicationUpdateTransition>> {
    let mut transitions = Vec::new();
    for installation in installations {
        if installation.source_layer != crate::skill_state::SkillLayerRecord::Managed {
            continue;
        }
        let Some(revision_id) = installation.active_revision_id.as_deref() else {
            continue;
        };
        let active = candidate
            .packages()
            .iter()
            .any(|resolved| resolved_revision_is(resolved, &installation.package_id, revision_id));
        if active {
            continue;
        }
        let reason = candidate
            .inactive()
            .iter()
            .find(|resolved| resolved_revision_is(resolved, &installation.package_id, revision_id))
            .map(|resolved| resolved.reason.clone())
            .context("application update lost an active managed revision without a diagnostic")?;
        transitions.push(ApplicationUpdateTransition {
            installation,
            reason,
        });
    }
    transitions.sort_by(|left, right| {
        left.installation
            .package_id
            .cmp(&right.installation.package_id)
    });
    Ok(transitions)
}

fn resolved_revision_is(
    resolved: &ResolvedSkillPackage,
    package_id: &SkillPackageId,
    revision_id: &str,
) -> bool {
    resolved.package.descriptor.id == *package_id
        && resolved
            .package
            .verified_content
            .as_ref()
            .and_then(|content| content.execution_binding.as_ref())
            .is_some_and(|binding| binding.revision_id == revision_id)
}

#[allow(dead_code)]
fn _graph_state_type_anchor(_: &ApplicationGraphState) {}
