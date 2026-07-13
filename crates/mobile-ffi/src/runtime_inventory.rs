use crate::types::MobileSkillDto;
use agent_runtime::skill_management::{
    LayeredSkillInventoryItem, SkillActionFacts, SkillPackageStatus,
};
use agent_runtime::skill_package::SkillPackageId;
use agent_runtime::skill_resolver::{ResolvedSkillPackage, SkillResolutionStatus};
use agent_runtime::skill_source::SkillLayer;
use anyhow::{Context, Result};
use std::collections::BTreeMap;

pub(super) fn mobile_layered_skill(item: LayeredSkillInventoryItem) -> Result<MobileSkillDto> {
    let primary = item
        .effective
        .as_ref()
        .or(item.managed.as_ref())
        .context("layered mobile inventory item has no layer")?;
    Ok(MobileSkillDto {
        package_id: item.package_id.as_str().to_string(),
        display_name: primary.display_name.clone(),
        version: primary.version.clone(),
        source_layer: primary.source_layer.clone(),
        status: primary.status.clone(),
        available: primary.available,
        reason: primary.reason.clone(),
        active_revision_id: primary.active_revision_id.clone(),
        manageable: item.actions != SkillActionFacts::default(),
        built_in_collision: item.built_in_collision,
        effective: item.effective,
        managed: item.managed,
        actions: item.actions,
    })
}

pub(super) fn mobile_layer_status(
    resolved: &ResolvedSkillPackage,
    available: bool,
    active_revision_id: Option<String>,
    manageable: bool,
) -> SkillPackageStatus {
    SkillPackageStatus {
        package_id: resolved.package.descriptor.id.clone(),
        display_name: resolved.package.descriptor.display_name.clone(),
        version: resolved.package.descriptor.version.to_string(),
        source_layer: layer_name(resolved.package.layer).into(),
        status: resolution_status_name(resolved.status).into(),
        reason: resolved.reason.clone(),
        active_revision_id,
        available,
        content_hash: Some(resolved.package.content_hash.clone()),
        manageable,
    }
}

pub(super) fn managed_revision_ids(
    value: &serde_json::Value,
) -> Result<BTreeMap<SkillPackageId, String>> {
    let members = value
        .as_array()
        .context("active skill snapshot members must be an array")?;
    let mut revisions = BTreeMap::new();
    for member in members {
        if member.get("layer").and_then(serde_json::Value::as_str) != Some("managed") {
            continue;
        }
        let package_id = member
            .get("packageId")
            .and_then(serde_json::Value::as_str)
            .context("managed snapshot member is missing packageId")?;
        let revision_id = member
            .get("revisionId")
            .and_then(serde_json::Value::as_str)
            .context("managed snapshot member is missing revisionId")?;
        revisions.insert(SkillPackageId::parse(package_id)?, revision_id.to_string());
    }
    Ok(revisions)
}

pub(super) fn managed_inventory_revision(
    resolved: &ResolvedSkillPackage,
    active: bool,
    active_revisions: &BTreeMap<SkillPackageId, String>,
    managed_revisions: &BTreeMap<SkillPackageId, (String, String, String)>,
) -> Result<Option<String>> {
    if resolved.package.layer != SkillLayer::Managed {
        return Ok(None);
    }
    let package_id = &resolved.package.descriptor.id;
    let (authoritative_revision, authoritative_version, authoritative_content_hash) =
        managed_revisions.get(package_id).with_context(|| {
            format!(
                "managed inventory state is missing for {}",
                package_id.as_str()
            )
        })?;
    anyhow::ensure!(
        authoritative_version == &resolved.package.descriptor.version.to_string(),
        "managed inventory version is inconsistent for {}",
        package_id.as_str()
    );
    anyhow::ensure!(
        authoritative_content_hash == &resolved.package.content_hash,
        "managed inventory content hash is inconsistent for {}",
        package_id.as_str()
    );
    if active {
        let generation_revision = active_revisions.get(package_id).with_context(|| {
            format!(
                "active snapshot revision is missing for {}",
                package_id.as_str()
            )
        })?;
        anyhow::ensure!(
            generation_revision == authoritative_revision,
            "active snapshot revision is stale for {}",
            package_id.as_str()
        );
    }
    Ok(Some(authoritative_revision.clone()))
}

pub(super) fn layer_name(layer: SkillLayer) -> &'static str {
    match layer {
        SkillLayer::Builtin => "builtin",
        SkillLayer::Managed => "managed",
        SkillLayer::Session => "session",
    }
}

pub(super) fn resolution_status_name(status: SkillResolutionStatus) -> &'static str {
    match status {
        SkillResolutionStatus::Active => "active",
        SkillResolutionStatus::Overridden => "overridden",
        SkillResolutionStatus::OverrideDenied => "override_denied",
        SkillResolutionStatus::ProtectedPackage => "protected_package",
        SkillResolutionStatus::DependencyMissing => "dependency_missing",
        SkillResolutionStatus::CapabilityMissing => "capability_missing",
        SkillResolutionStatus::PlatformUnsupported => "platform_unsupported",
        SkillResolutionStatus::RuntimeIncompatible => "runtime_incompatible",
        SkillResolutionStatus::CircuitOpen => "circuit_open",
        SkillResolutionStatus::NetworkPolicyUnavailable => "network_policy_unavailable",
    }
}
