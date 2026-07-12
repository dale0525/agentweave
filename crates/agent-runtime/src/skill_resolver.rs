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
    pub fn resolve(input: SkillResolutionInput) -> anyhow::Result<ResolvedSkillSet> {
        let SkillResolutionInput {
            mut packages,
            platform,
            capabilities,
            protected_packages,
            allowed_overrides,
            runtime_version,
        } = input;
        packages.sort_by(package_order);
        let protected: BTreeSet<_> = protected_packages.into_iter().collect();
        let overrides: BTreeSet<_> = allowed_overrides.into_iter().collect();
        let mut grouped =
            BTreeMap::<SkillPackageId, BTreeMap<SkillLayer, DiscoveredSkillPackage>>::new();

        for package in packages {
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

        let platform_name = platform_name(platform);
        let mut active = BTreeMap::new();
        let mut inactive = Vec::new();
        for (id, mut candidates) in grouped {
            let has_builtin = candidates.contains_key(&SkillLayer::Builtin);
            let has_managed = candidates.contains_key(&SkillLayer::Managed);
            let has_persistent = has_builtin || has_managed;
            let builtin = locally_eligible(
                candidates.remove(&SkillLayer::Builtin),
                platform_name,
                &capabilities,
                &runtime_version,
                &mut inactive,
            );
            let managed = locally_eligible(
                candidates.remove(&SkillLayer::Managed),
                platform_name,
                &capabilities,
                &runtime_version,
                &mut inactive,
            );
            let session = locally_eligible(
                candidates.remove(&SkillLayer::Session),
                platform_name,
                &capabilities,
                &runtime_version,
                &mut inactive,
            );

            let persistent = match (builtin, managed) {
                (Some(builtin), Some(managed)) => select_managed_override(
                    Some(builtin),
                    managed,
                    &id,
                    &protected,
                    &overrides,
                    &mut inactive,
                ),
                (None, Some(managed)) if has_builtin => select_managed_override(
                    None,
                    managed,
                    &id,
                    &protected,
                    &overrides,
                    &mut inactive,
                ),
                (None, Some(managed)) => Some(ActiveCandidate::without_fallback(managed)),
                (Some(builtin), None) => Some(ActiveCandidate::without_fallback(builtin)),
                (None, None) => None,
            };

            let selected = if has_persistent {
                if let Some(session) = session {
                    inactive.push(resolved(
                        session,
                        SkillResolutionStatus::OverrideDenied,
                        "session package cannot override a built-in or managed package",
                    ));
                }
                persistent
            } else {
                debug_assert!(!has_managed);
                session.map(ActiveCandidate::without_fallback)
            };
            if let Some(selected) = selected {
                active.insert(id, selected);
            }
        }

        resolve_dependencies(&mut active, &mut inactive);

        let mut resolved_active = Vec::with_capacity(active.len());
        for (_, candidate) in active {
            if let Some(overridden) = candidate.fallback {
                inactive.push(resolved(
                    overridden,
                    SkillResolutionStatus::Overridden,
                    "built-in package was overridden by an allowed managed package",
                ));
            }
            resolved_active.push(resolved(
                candidate.package,
                SkillResolutionStatus::Active,
                "active",
            ));
        }
        inactive.sort_by(resolution_order);
        Ok(ResolvedSkillSet {
            active: resolved_active,
            inactive,
        })
    }
}

#[derive(Debug)]
struct ActiveCandidate {
    package: DiscoveredSkillPackage,
    fallback: Option<DiscoveredSkillPackage>,
}

impl ActiveCandidate {
    fn without_fallback(package: DiscoveredSkillPackage) -> Self {
        Self {
            package,
            fallback: None,
        }
    }
}

fn select_managed_override(
    builtin: Option<DiscoveredSkillPackage>,
    managed: DiscoveredSkillPackage,
    id: &SkillPackageId,
    protected: &BTreeSet<SkillPackageId>,
    overrides: &BTreeSet<SkillPackageId>,
    inactive: &mut Vec<ResolvedSkillPackage>,
) -> Option<ActiveCandidate> {
    if overrides.contains(id) {
        return Some(ActiveCandidate {
            package: managed,
            fallback: builtin,
        });
    }

    let (status, reason) = if protected.contains(id) {
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
    builtin.map(ActiveCandidate::without_fallback)
}

fn locally_eligible(
    package: Option<DiscoveredSkillPackage>,
    platform_name: &str,
    capabilities: &CapabilitySet,
    runtime_version: &Version,
    inactive: &mut Vec<ResolvedSkillPackage>,
) -> Option<DiscoveredSkillPackage> {
    let package = package?;
    if let Some((status, reason)) =
        local_failure(&package, platform_name, capabilities, runtime_version)
    {
        inactive.push(resolved(package, status, reason));
        None
    } else {
        Some(package)
    }
}

fn local_failure(
    package: &DiscoveredSkillPackage,
    platform_name: &str,
    capabilities: &CapabilitySet,
    runtime_version: &Version,
) -> Option<(SkillResolutionStatus, String)> {
    let descriptor = &package.descriptor;
    let runtime_blocked = descriptor
        .compatibility
        .minimum_runtime_version
        .as_ref()
        .is_some_and(|minimum| minimum > runtime_version);
    if runtime_blocked {
        return Some((
            SkillResolutionStatus::RuntimeIncompatible,
            format!("runtime {runtime_version} is below the package minimum"),
        ));
    }

    let platform_blocked = !descriptor.compatibility.platforms.is_empty()
        && !descriptor
            .compatibility
            .platforms
            .iter()
            .any(|name| name.eq_ignore_ascii_case(platform_name));
    if platform_blocked {
        return Some((
            SkillResolutionStatus::PlatformUnsupported,
            format!("unsupported platform: {platform_name}"),
        ));
    }

    descriptor
        .requires
        .capabilities
        .iter()
        .filter(|name| !capabilities.contains_name(name))
        .min()
        .map(|name| {
            (
                SkillResolutionStatus::CapabilityMissing,
                format!("missing capability: {name}"),
            )
        })
}

fn resolve_dependencies(
    active: &mut BTreeMap<SkillPackageId, ActiveCandidate>,
    inactive: &mut Vec<ResolvedSkillPackage>,
) {
    let mut reverse = BTreeMap::<SkillPackageId, BTreeSet<SkillPackageId>>::new();
    for (id, candidate) in active.iter() {
        add_dependency_edges(&mut reverse, id, &candidate.package);
    }

    let mut pending = active
        .iter()
        .filter_map(|(id, candidate)| {
            missing_dependency(&candidate.package, active).map(|_| id.clone())
        })
        .collect::<BTreeSet<_>>();

    while let Some(id) = pending.pop_first() {
        let Some(missing) = active
            .get(&id)
            .and_then(|candidate| missing_dependency(&candidate.package, active))
        else {
            continue;
        };
        let mut candidate = active
            .remove(&id)
            .expect("pending dependency resolution must reference an active package");
        remove_dependency_edges(&mut reverse, &id, &candidate.package);
        inactive.push(resolved(
            candidate.package,
            SkillResolutionStatus::DependencyMissing,
            format!("missing dependency: {}", missing.as_str()),
        ));

        if let Some(fallback) = candidate.fallback.take() {
            let replacement = ActiveCandidate::without_fallback(fallback);
            add_dependency_edges(&mut reverse, &id, &replacement.package);
            active.insert(id.clone(), replacement);
            if active
                .get(&id)
                .and_then(|replacement| missing_dependency(&replacement.package, active))
                .is_some()
            {
                pending.insert(id);
            }
            continue;
        }

        if let Some(dependents) = reverse.remove(&id) {
            pending.extend(
                dependents
                    .into_iter()
                    .filter(|dependent| active.contains_key(dependent)),
            );
        }
    }
}

fn missing_dependency(
    package: &DiscoveredSkillPackage,
    active: &BTreeMap<SkillPackageId, ActiveCandidate>,
) -> Option<SkillPackageId> {
    package
        .descriptor
        .requires
        .packages
        .iter()
        .filter(|dependency| !active.contains_key(*dependency))
        .min()
        .cloned()
}

fn add_dependency_edges(
    reverse: &mut BTreeMap<SkillPackageId, BTreeSet<SkillPackageId>>,
    package_id: &SkillPackageId,
    package: &DiscoveredSkillPackage,
) {
    for dependency in &package.descriptor.requires.packages {
        reverse
            .entry(dependency.clone())
            .or_default()
            .insert(package_id.clone());
    }
}

fn remove_dependency_edges(
    reverse: &mut BTreeMap<SkillPackageId, BTreeSet<SkillPackageId>>,
    package_id: &SkillPackageId,
    package: &DiscoveredSkillPackage,
) {
    for dependency in &package.descriptor.requires.packages {
        let remove_entry = reverse.get_mut(dependency).is_some_and(|dependents| {
            dependents.remove(package_id);
            dependents.is_empty()
        });
        if remove_entry {
            reverse.remove(dependency);
        }
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
