use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_package::{
    SkillCompatibility, SkillPackageDescriptor, SkillPackageId, SkillPackageKind,
    SkillPackageRequirements, SkillPackageTargets,
};
use crate::skill_resolver::{SkillResolutionInput, SkillResolutionStatus, SkillResolver};
use crate::skill_source::{
    DirectorySkillSource, DiscoveredSkillPackage, SkillLayer, SkillSource, hash_package_tree,
};
use semver::Version;
use std::path::{Path, PathBuf};

fn package(id: &str, layer: SkillLayer) -> DiscoveredSkillPackage {
    DiscoveredSkillPackage {
        layer,
        root: PathBuf::from(format!("/{layer:?}/{id}")),
        descriptor: SkillPackageDescriptor {
            schema_version: 1,
            id: SkillPackageId::parse(id).unwrap(),
            version: Version::new(1, 0, 0),
            display_name: id.into(),
            kind: SkillPackageKind::InstructionOnly,
            package: SkillPackageTargets {
                include_instructions: true,
                include_runtime: false,
            },
            compatibility: SkillCompatibility::default(),
            requires: SkillPackageRequirements::default(),
        },
        content_hash: format!("hash-{id}-{layer:?}"),
        warnings: Vec::new(),
    }
}

fn input(packages: Vec<DiscoveredSkillPackage>) -> SkillResolutionInput {
    SkillResolutionInput {
        packages,
        platform: PlatformId::Desktop,
        capabilities: CapabilitySet::from_names(Vec::<String>::new()),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: Version::new(0, 3, 0),
    }
}

fn status_for(
    resolved: &[crate::skill_resolver::ResolvedSkillPackage],
    id: &str,
) -> SkillResolutionStatus {
    resolved
        .iter()
        .find(|item| item.package.descriptor.id.as_str() == id)
        .unwrap_or_else(|| panic!("missing resolution for {id}"))
        .status
}

#[test]
fn builtin_package_wins_without_explicit_override() {
    let resolved = SkillResolver::resolve(input(vec![
        package("com.example.calendar", SkillLayer::Managed),
        package("com.example.calendar", SkillLayer::Builtin),
    ]))
    .unwrap();

    assert_eq!(resolved.active.len(), 1);
    assert_eq!(resolved.active[0].package.layer, SkillLayer::Builtin);
    assert!(resolved.inactive.iter().any(|item| {
        item.package.layer == SkillLayer::Managed
            && item.status == SkillResolutionStatus::OverrideDenied
    }));
}

#[test]
fn allowed_managed_override_retains_builtin_diagnostic() {
    let id = SkillPackageId::parse("com.example.calendar").unwrap();
    let mut resolution_input = input(vec![
        package(id.as_str(), SkillLayer::Session),
        package(id.as_str(), SkillLayer::Builtin),
        package(id.as_str(), SkillLayer::Managed),
    ]);
    resolution_input.allowed_overrides = vec![id];

    let resolved = SkillResolver::resolve(resolution_input).unwrap();

    assert_eq!(resolved.active.len(), 1);
    assert_eq!(resolved.active[0].package.layer, SkillLayer::Managed);
    assert!(resolved.inactive.iter().any(|item| {
        item.package.layer == SkillLayer::Builtin
            && item.status == SkillResolutionStatus::Overridden
    }));
    assert!(resolved.inactive.iter().any(|item| {
        item.package.layer == SkillLayer::Session
            && item.status == SkillResolutionStatus::OverrideDenied
    }));
}

#[test]
fn protected_builtin_rejects_allowed_managed_override() {
    let id = SkillPackageId::parse("com.example.calendar").unwrap();
    let mut resolution_input = input(vec![
        package(id.as_str(), SkillLayer::Managed),
        package(id.as_str(), SkillLayer::Builtin),
    ]);
    resolution_input.allowed_overrides = vec![id.clone()];
    resolution_input.protected_packages = vec![id];

    let resolved = SkillResolver::resolve(resolution_input).unwrap();

    assert_eq!(resolved.active[0].package.layer, SkillLayer::Builtin);
    assert_eq!(
        resolved.inactive[0].status,
        SkillResolutionStatus::ProtectedPackage
    );
}

#[test]
fn session_package_activates_only_without_persistent_candidate() {
    let resolved = SkillResolver::resolve(input(vec![
        package("com.example.session-only", SkillLayer::Session),
        package("com.example.managed", SkillLayer::Session),
        package("com.example.managed", SkillLayer::Managed),
        package("com.example.builtin", SkillLayer::Session),
        package("com.example.builtin", SkillLayer::Builtin),
    ]))
    .unwrap();

    assert!(resolved.active.iter().any(|item| {
        item.package.descriptor.id.as_str() == "com.example.session-only"
            && item.package.layer == SkillLayer::Session
    }));
    assert_eq!(
        resolved
            .inactive
            .iter()
            .filter(|item| item.package.layer == SkillLayer::Session)
            .count(),
        2
    );
}

#[test]
fn missing_dependency_disables_only_the_dependent_package() {
    let mut dependent = package("com.example.calendar", SkillLayer::Managed);
    dependent.descriptor.requires.packages =
        vec![SkillPackageId::parse("com.example.provider").unwrap()];
    let independent = package("com.example.notes", SkillLayer::Managed);

    let resolved = SkillResolver::resolve(input(vec![dependent, independent])).unwrap();

    assert_eq!(resolved.active.len(), 1);
    assert_eq!(
        resolved.active[0].package.descriptor.id.as_str(),
        "com.example.notes"
    );
    assert_eq!(
        resolved.inactive[0].status,
        SkillResolutionStatus::DependencyMissing
    );
}

#[test]
fn dependency_inactivation_propagates_to_a_fixed_point() {
    let mut capability_provider = package("com.example.capability", SkillLayer::Managed);
    capability_provider.descriptor.requires.capabilities = vec!["network.http".into()];

    let mut platform_provider = package("com.example.platform", SkillLayer::Managed);
    platform_provider.descriptor.compatibility.platforms = vec!["server".into()];

    let mut runtime_provider = package("com.example.runtime", SkillLayer::Managed);
    runtime_provider
        .descriptor
        .compatibility
        .minimum_runtime_version = Some(Version::new(9, 0, 0));

    let mut capability_dependent = package("com.example.needs-capability", SkillLayer::Managed);
    capability_dependent.descriptor.requires.packages =
        vec![SkillPackageId::parse("com.example.capability").unwrap()];

    let mut platform_dependent = package("com.example.needs-platform", SkillLayer::Managed);
    platform_dependent.descriptor.requires.packages =
        vec![SkillPackageId::parse("com.example.platform").unwrap()];

    let mut runtime_dependent = package("com.example.needs-runtime", SkillLayer::Managed);
    runtime_dependent.descriptor.requires.packages =
        vec![SkillPackageId::parse("com.example.runtime").unwrap()];

    let mut transitive = package("com.example.transitive", SkillLayer::Managed);
    transitive.descriptor.requires.packages =
        vec![SkillPackageId::parse("com.example.needs-capability").unwrap()];

    let resolved = SkillResolver::resolve(input(vec![
        transitive,
        runtime_dependent,
        capability_provider,
        platform_dependent,
        runtime_provider,
        capability_dependent,
        platform_provider,
    ]))
    .unwrap();

    assert!(resolved.active.is_empty());
    assert_eq!(
        status_for(&resolved.inactive, "com.example.capability"),
        SkillResolutionStatus::CapabilityMissing
    );
    assert_eq!(
        status_for(&resolved.inactive, "com.example.platform"),
        SkillResolutionStatus::PlatformUnsupported
    );
    assert_eq!(
        status_for(&resolved.inactive, "com.example.runtime"),
        SkillResolutionStatus::RuntimeIncompatible
    );
    for id in [
        "com.example.needs-capability",
        "com.example.needs-platform",
        "com.example.needs-runtime",
        "com.example.transitive",
    ] {
        assert_eq!(
            status_for(&resolved.inactive, id),
            SkillResolutionStatus::DependencyMissing
        );
    }
}

#[test]
fn resolver_rejects_invalid_programmatic_descriptor() {
    let mut invalid = package("com.example.invalid", SkillLayer::Managed);
    invalid.descriptor.schema_version = 999;

    let error = SkillResolver::resolve(input(vec![invalid])).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("unsupported skill package schema")
    );
}

#[test]
fn resolver_rejects_duplicate_ids_within_the_same_layer() {
    let mut duplicate = package("com.example.duplicate", SkillLayer::Managed);
    duplicate.root = PathBuf::from("/other/com.example.duplicate");

    let error = SkillResolver::resolve(input(vec![
        package("com.example.duplicate", SkillLayer::Managed),
        duplicate,
    ]))
    .unwrap_err();

    let message = error.to_string();
    assert!(message.contains("duplicate package id"));
    assert!(message.contains("com.example.duplicate"));
}

#[test]
fn resolution_order_does_not_depend_on_input_order() {
    let packages = vec![
        package("com.example.zeta", SkillLayer::Managed),
        package("com.example.alpha", SkillLayer::Session),
        package("com.example.alpha", SkillLayer::Builtin),
        package("com.example.middle", SkillLayer::Managed),
    ];
    let mut reversed = packages.clone();
    reversed.reverse();

    let forward = SkillResolver::resolve(input(packages)).unwrap();
    let backward = SkillResolver::resolve(input(reversed)).unwrap();

    let project = |resolved: crate::skill_resolver::ResolvedSkillSet| {
        let active = resolved
            .active
            .into_iter()
            .map(|item| {
                (
                    item.package.descriptor.id.as_str().to_string(),
                    item.package.layer,
                    item.status,
                )
            })
            .collect::<Vec<_>>();
        let inactive = resolved
            .inactive
            .into_iter()
            .map(|item| {
                (
                    item.package.descriptor.id.as_str().to_string(),
                    item.package.layer,
                    item.status,
                )
            })
            .collect::<Vec<_>>();
        (active, inactive)
    };

    assert_eq!(project(forward), project(backward));
}

#[tokio::test]
async fn directory_source_sorts_packages_and_hashes_content_stably() {
    let temporary = tempfile::tempdir().unwrap();
    write_legacy_instruction_package(temporary.path(), "zeta", "zeta").await;
    write_legacy_instruction_package(temporary.path(), "alpha", "alpha").await;
    let source = DirectorySkillSource::new(SkillLayer::Managed, temporary.path());

    let first = source.discover().await.unwrap();
    let second = source.discover().await.unwrap();

    assert_eq!(source.layer(), SkillLayer::Managed);
    assert_eq!(
        first
            .iter()
            .map(|item| item.descriptor.id.as_str())
            .collect::<Vec<_>>(),
        vec!["legacy.local.alpha", "legacy.local.zeta"]
    );
    assert_eq!(
        first
            .iter()
            .map(|item| item.content_hash.as_str())
            .collect::<Vec<_>>(),
        second
            .iter()
            .map(|item| item.content_hash.as_str())
            .collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn directory_source_rejects_duplicate_package_ids() {
    let temporary = tempfile::tempdir().unwrap();
    write_explicit_instruction_package(temporary.path(), "first", "com.example.duplicate").await;
    write_explicit_instruction_package(temporary.path(), "second", "com.example.duplicate").await;
    let source = DirectorySkillSource::new(SkillLayer::Builtin, temporary.path());

    let error = source.discover().await.unwrap_err();

    assert!(error.to_string().contains("duplicate package id"));
}

#[tokio::test]
async fn package_tree_hash_is_independent_of_file_creation_order() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    write_tree(first.path(), false).await;
    write_tree(second.path(), true).await;

    assert_eq!(
        hash_package_tree(first.path()).await.unwrap(),
        hash_package_tree(second.path()).await.unwrap()
    );
}

#[cfg(unix)]
#[tokio::test]
async fn package_tree_hash_rejects_symlinks() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    tokio::fs::write(temporary.path().join("outside.txt"), b"outside")
        .await
        .unwrap();
    symlink(
        temporary.path().join("outside.txt"),
        temporary.path().join("linked.txt"),
    )
    .unwrap();

    let error = hash_package_tree(temporary.path()).await.unwrap_err();

    assert!(error.to_string().contains("cannot contain symlinks"));
}

async fn write_legacy_instruction_package(root: &Path, folder: &str, instructions: &str) {
    let package_root = root.join(folder);
    tokio::fs::create_dir_all(&package_root).await.unwrap();
    tokio::fs::write(package_root.join("SKILL.md"), instructions)
        .await
        .unwrap();
}

async fn write_explicit_instruction_package(root: &Path, folder: &str, id: &str) {
    let package_root = root.join(folder);
    tokio::fs::create_dir_all(&package_root).await.unwrap();
    let descriptor = serde_json::json!({
        "schemaVersion": 1,
        "id": id,
        "version": "1.0.0",
        "displayName": id,
        "kind": "instruction_only",
        "package": {
            "includeInstructions": true,
            "includeRuntime": false
        }
    });
    tokio::fs::write(
        package_root.join("general-agent.json"),
        serde_json::to_vec(&descriptor).unwrap(),
    )
    .await
    .unwrap();
    tokio::fs::write(package_root.join("SKILL.md"), "instructions")
        .await
        .unwrap();
}

async fn write_tree(root: &Path, reverse: bool) {
    tokio::fs::create_dir_all(root.join("nested"))
        .await
        .unwrap();
    let files = if reverse {
        [("nested/two.txt", "two"), ("one.txt", "one")]
    } else {
        [("one.txt", "one"), ("nested/two.txt", "two")]
    };
    for (path, contents) in files {
        tokio::fs::write(root.join(path), contents).await.unwrap();
    }
}
