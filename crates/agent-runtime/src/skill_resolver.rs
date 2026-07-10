use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_package::SkillPackageId;
use crate::skill_source::{DiscoveredSkillPackage, SkillLayer};
use semver::Version;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum SkillResolutionStatus {
    Active,
    Overridden,
    OverrideDenied,
    ProtectedPackage,
    DependencyMissing,
    CapabilityMissing,
    PlatformUnsupported,
    RuntimeIncompatible,
}

#[derive(Clone, Debug)]
pub struct ResolvedSkillPackage {
    pub package: DiscoveredSkillPackage,
    pub status: SkillResolutionStatus,
    pub reason: String,
}

#[derive(Clone, Debug)]
pub struct ResolvedSkillSet {
    pub active: Vec<ResolvedSkillPackage>,
    pub inactive: Vec<ResolvedSkillPackage>,
}

pub struct SkillResolutionInput {
    pub packages: Vec<DiscoveredSkillPackage>,
    pub platform: PlatformId,
    pub capabilities: CapabilitySet,
    pub protected_packages: Vec<SkillPackageId>,
    pub allowed_overrides: Vec<SkillPackageId>,
    pub runtime_version: Version,
}

pub struct SkillResolver;

impl SkillResolver {
    pub fn resolve(mut input: SkillResolutionInput) -> anyhow::Result<ResolvedSkillSet> {
        input.packages.sort_by(package_order);
        let protected: BTreeSet<_> = input.protected_packages.into_iter().collect();
        let overrides: BTreeSet<_> = input.allowed_overrides.into_iter().collect();
        let mut grouped =
            BTreeMap::<SkillPackageId, BTreeMap<SkillLayer, DiscoveredSkillPackage>>::new();

        for package in input.packages {
            if let Err(error) = package.descriptor.validate() {
                anyhow::bail!(
                    "invalid descriptor for package {} at {}: {error}",
                    package.descriptor.id.as_str(),
                    package.root.display()
                );
            }
            let id = package.descriptor.id.clone();
            let layer = package.layer;
            let root = package.root.clone();
            if let Some(previous) = grouped
                .entry(id.clone())
                .or_default()
                .insert(layer, package)
            {
                anyhow::bail!(
                    "duplicate package id {} in {:?} layer: {} and {}",
                    id.as_str(),
                    layer,
                    previous.root.display(),
                    root.display()
                );
            }
        }

        let mut winners = Vec::new();
        let mut inactive = Vec::new();
        for (id, mut candidates) in grouped {
            let builtin = candidates.remove(&SkillLayer::Builtin);
            let managed = candidates.remove(&SkillLayer::Managed);
            let mut session = candidates.remove(&SkillLayer::Session);

            let winner = match (builtin, managed) {
                (Some(builtin), Some(managed))
                    if overrides.contains(&id) && !protected.contains(&id) =>
                {
                    inactive.push(resolved(
                        builtin,
                        SkillResolutionStatus::Overridden,
                        "built-in package was overridden by an allowed managed package",
                    ));
                    managed
                }
                (Some(builtin), Some(managed)) => {
                    let (status, reason) = if protected.contains(&id) {
                        (
                            SkillResolutionStatus::ProtectedPackage,
                            "managed package cannot override a protected built-in package",
                        )
                    } else {
                        (
                            SkillResolutionStatus::OverrideDenied,
                            "managed package cannot override a built-in package without permission",
                        )
                    };
                    inactive.push(resolved(managed, status, reason));
                    builtin
                }
                (Some(builtin), None) => builtin,
                (None, Some(managed)) => managed,
                (None, None) => session
                    .take()
                    .expect("a grouped package must contain at least one candidate"),
            };

            if let Some(session) = session
                && winner.layer != SkillLayer::Session
            {
                inactive.push(resolved(
                    session,
                    SkillResolutionStatus::OverrideDenied,
                    "session package cannot override a built-in or managed package",
                ));
            }
            winners.push(resolved(winner, SkillResolutionStatus::Active, "active"));
        }

        let platform_name = platform_name(input.platform);
        let mut eligible = BTreeMap::new();
        for winner in winners {
            let descriptor = &winner.package.descriptor;
            let runtime_blocked = descriptor
                .compatibility
                .minimum_runtime_version
                .as_ref()
                .is_some_and(|minimum| minimum > &input.runtime_version);
            let platform_blocked = !descriptor.compatibility.platforms.is_empty()
                && !descriptor
                    .compatibility
                    .platforms
                    .iter()
                    .any(|name| name.eq_ignore_ascii_case(platform_name));
            let missing_capability = descriptor
                .requires
                .capabilities
                .iter()
                .filter(|name| !input.capabilities.contains_name(name))
                .min();

            let failure = if runtime_blocked {
                Some((
                    SkillResolutionStatus::RuntimeIncompatible,
                    format!(
                        "runtime {} is below the package minimum",
                        input.runtime_version
                    ),
                ))
            } else if platform_blocked {
                Some((
                    SkillResolutionStatus::PlatformUnsupported,
                    format!("unsupported platform: {platform_name}"),
                ))
            } else {
                missing_capability.map(|name| {
                    (
                        SkillResolutionStatus::CapabilityMissing,
                        format!("missing capability: {name}"),
                    )
                })
            };

            if let Some((status, reason)) = failure {
                inactive.push(resolved(winner.package, status, reason));
            } else {
                eligible.insert(descriptor.id.clone(), winner);
            }
        }

        loop {
            let active_ids: BTreeSet<_> = eligible.keys().cloned().collect();
            let removals = eligible
                .iter()
                .filter_map(|(id, item)| {
                    item.package
                        .descriptor
                        .requires
                        .packages
                        .iter()
                        .filter(|dependency| !active_ids.contains(*dependency))
                        .min()
                        .cloned()
                        .map(|dependency| (id.clone(), dependency))
                })
                .collect::<Vec<_>>();
            if removals.is_empty() {
                break;
            }
            for (id, dependency) in removals {
                let item = eligible
                    .remove(&id)
                    .expect("dependency removal must reference an eligible package");
                inactive.push(resolved(
                    item.package,
                    SkillResolutionStatus::DependencyMissing,
                    format!("missing dependency: {}", dependency.as_str()),
                ));
            }
        }

        let active = eligible.into_values().collect();
        inactive.sort_by(resolution_order);
        Ok(ResolvedSkillSet { active, inactive })
    }
}

fn platform_name(platform: PlatformId) -> &'static str {
    match platform {
        PlatformId::Desktop => "desktop",
        PlatformId::Android => "android",
        PlatformId::Ios => "ios",
        PlatformId::Web => "web",
        PlatformId::Server => "server",
    }
}

fn resolved(
    package: DiscoveredSkillPackage,
    status: SkillResolutionStatus,
    reason: impl Into<String>,
) -> ResolvedSkillPackage {
    ResolvedSkillPackage {
        package,
        status,
        reason: reason.into(),
    }
}

fn package_order(
    left: &DiscoveredSkillPackage,
    right: &DiscoveredSkillPackage,
) -> std::cmp::Ordering {
    left.descriptor
        .id
        .cmp(&right.descriptor.id)
        .then_with(|| left.layer.cmp(&right.layer))
        .then_with(|| left.root.cmp(&right.root))
}

fn resolution_order(
    left: &ResolvedSkillPackage,
    right: &ResolvedSkillPackage,
) -> std::cmp::Ordering {
    package_order(&left.package, &right.package).then_with(|| left.status.cmp(&right.status))
}
