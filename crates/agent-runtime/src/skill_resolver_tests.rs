use crate::platform::{CapabilitySet, PlatformId};
use crate::skill_package::{
    SkillCompatibility, SkillPackageDescriptor, SkillPackageId, SkillPackageKind,
    SkillPackageRequirements, SkillPackageTargets,
};
use crate::skill_resolver::{SkillResolutionInput, SkillResolutionStatus, SkillResolver};
use crate::skill_source::{
    DirectorySkillSource, DiscoveredSkillPackage, SkillLayer, SkillSource, canonical_relative_path,
    hash_package_tree, portable_collision_key,
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
        verified_content: None,
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

fn contains_resolution(
    resolved: &[crate::skill_resolver::ResolvedSkillPackage],
    id: &str,
    layer: SkillLayer,
    status: SkillResolutionStatus,
) -> bool {
    resolved.iter().any(|item| {
        item.package.descriptor.id.as_str() == id
            && item.package.layer == layer
            && item.status == status
    })
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
fn runtime_ineligible_managed_override_falls_back_to_builtin() {
    let id = SkillPackageId::parse("com.example.calendar").unwrap();
    let mut managed = package(id.as_str(), SkillLayer::Managed);
    managed.descriptor.compatibility.minimum_runtime_version = Some(Version::new(9, 0, 0));
    let mut resolution_input = input(vec![package(id.as_str(), SkillLayer::Builtin), managed]);
    resolution_input.allowed_overrides = vec![id.clone()];

    let resolved = SkillResolver::resolve(resolution_input).unwrap();

    assert_eq!(resolved.active[0].package.layer, SkillLayer::Builtin);
    assert!(contains_resolution(
        &resolved.inactive,
        id.as_str(),
        SkillLayer::Managed,
        SkillResolutionStatus::RuntimeIncompatible
    ));
    assert!(
        !resolved
            .inactive
            .iter()
            .any(|item| item.status == SkillResolutionStatus::Overridden)
    );
}

#[test]
fn capability_ineligible_managed_override_falls_back_to_builtin() {
    let id = SkillPackageId::parse("com.example.calendar").unwrap();
    let mut managed = package(id.as_str(), SkillLayer::Managed);
    managed.descriptor.requires.capabilities = vec!["network.http".into()];
    let mut resolution_input = input(vec![package(id.as_str(), SkillLayer::Builtin), managed]);
    resolution_input.allowed_overrides = vec![id.clone()];

    let resolved = SkillResolver::resolve(resolution_input).unwrap();

    assert_eq!(resolved.active[0].package.layer, SkillLayer::Builtin);
    assert!(contains_resolution(
        &resolved.inactive,
        id.as_str(),
        SkillLayer::Managed,
        SkillResolutionStatus::CapabilityMissing
    ));
}

#[test]
fn platform_ineligible_managed_override_falls_back_to_builtin() {
    let id = SkillPackageId::parse("com.example.calendar").unwrap();
    let mut managed = package(id.as_str(), SkillLayer::Managed);
    managed.descriptor.compatibility.platforms = vec!["server".into()];
    let mut resolution_input = input(vec![package(id.as_str(), SkillLayer::Builtin), managed]);
    resolution_input.allowed_overrides = vec![id.clone()];

    let resolved = SkillResolver::resolve(resolution_input).unwrap();

    assert_eq!(resolved.active[0].package.layer, SkillLayer::Builtin);
    assert!(contains_resolution(
        &resolved.inactive,
        id.as_str(),
        SkillLayer::Managed,
        SkillResolutionStatus::PlatformUnsupported
    ));
}

#[test]
fn dependency_ineligible_managed_override_falls_back_to_builtin() {
    let id = SkillPackageId::parse("com.example.calendar").unwrap();
    let mut managed = package(id.as_str(), SkillLayer::Managed);
    managed.descriptor.requires.packages =
        vec![SkillPackageId::parse("com.example.missing").unwrap()];
    let mut resolution_input = input(vec![package(id.as_str(), SkillLayer::Builtin), managed]);
    resolution_input.allowed_overrides = vec![id.clone()];

    let resolved = SkillResolver::resolve(resolution_input).unwrap();

    assert_eq!(resolved.active[0].package.layer, SkillLayer::Builtin);
    assert!(contains_resolution(
        &resolved.inactive,
        id.as_str(),
        SkillLayer::Managed,
        SkillResolutionStatus::DependencyMissing
    ));
    assert!(
        !resolved
            .inactive
            .iter()
            .any(|item| item.status == SkillResolutionStatus::Overridden)
    );
}

#[test]
fn dependency_fallback_rechecks_builtin_dependencies() {
    let alpha_id = SkillPackageId::parse("com.example.alpha").unwrap();
    let beta_id = SkillPackageId::parse("com.example.beta").unwrap();
    let mut alpha_builtin = package(alpha_id.as_str(), SkillLayer::Builtin);
    alpha_builtin.descriptor.requires.packages = vec![beta_id.clone()];
    let mut alpha_managed = package(alpha_id.as_str(), SkillLayer::Managed);
    alpha_managed.descriptor.requires.packages =
        vec![SkillPackageId::parse("com.example.missing-alpha").unwrap()];
    let mut beta = package(beta_id.as_str(), SkillLayer::Managed);
    beta.descriptor.requires.packages =
        vec![SkillPackageId::parse("com.example.missing-beta").unwrap()];
    let mut resolution_input = input(vec![beta, alpha_managed, alpha_builtin]);
    resolution_input.allowed_overrides = vec![alpha_id.clone()];

    let resolved = SkillResolver::resolve(resolution_input).unwrap();

    assert!(resolved.active.is_empty());
    assert!(contains_resolution(
        &resolved.inactive,
        alpha_id.as_str(),
        SkillLayer::Managed,
        SkillResolutionStatus::DependencyMissing
    ));
    assert!(contains_resolution(
        &resolved.inactive,
        alpha_id.as_str(),
        SkillLayer::Builtin,
        SkillResolutionStatus::DependencyMissing
    ));
}

#[test]
fn three_layer_dependency_fallback_reports_each_candidate_once() {
    let id = SkillPackageId::parse("com.example.calendar").unwrap();
    let mut managed = package(id.as_str(), SkillLayer::Managed);
    managed.descriptor.requires.packages =
        vec![SkillPackageId::parse("com.example.missing").unwrap()];
    let mut resolution_input = input(vec![
        package(id.as_str(), SkillLayer::Session),
        managed,
        package(id.as_str(), SkillLayer::Builtin),
    ]);
    resolution_input.allowed_overrides = vec![id.clone()];

    let resolved = SkillResolver::resolve(resolution_input).unwrap();

    assert_eq!(resolved.active.len(), 1);
    assert_eq!(resolved.active[0].package.layer, SkillLayer::Builtin);
    assert_eq!(resolved.inactive.len(), 2);
    assert!(contains_resolution(
        &resolved.inactive,
        id.as_str(),
        SkillLayer::Managed,
        SkillResolutionStatus::DependencyMissing
    ));
    assert!(contains_resolution(
        &resolved.inactive,
        id.as_str(),
        SkillLayer::Session,
        SkillResolutionStatus::OverrideDenied
    ));
}

#[test]
fn explicitly_allowed_protected_builtin_rejects_managed_override() {
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
fn session_cannot_replace_a_locally_ineligible_persistent_candidate() {
    let id = SkillPackageId::parse("com.example.calendar").unwrap();
    let mut builtin = package(id.as_str(), SkillLayer::Builtin);
    builtin.descriptor.compatibility.minimum_runtime_version = Some(Version::new(9, 0, 0));

    let resolved = SkillResolver::resolve(input(vec![
        package(id.as_str(), SkillLayer::Session),
        builtin,
    ]))
    .unwrap();

    assert!(resolved.active.is_empty());
    assert!(contains_resolution(
        &resolved.inactive,
        id.as_str(),
        SkillLayer::Builtin,
        SkillResolutionStatus::RuntimeIncompatible
    ));
    assert!(contains_resolution(
        &resolved.inactive,
        id.as_str(),
        SkillLayer::Session,
        SkillResolutionStatus::OverrideDenied
    ));
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

#[cfg(unix)]
#[tokio::test]
async fn directory_source_accepts_only_safe_directory_symlinks() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    write_explicit_instruction_package(temporary.path(), "host", "com.example.host").await;
    write_explicit_instruction_package(
        &temporary.path().join("host"),
        "linked-target",
        "com.example.linked",
    )
    .await;
    write_explicit_instruction_package(outside.path(), "outside", "com.example.outside").await;
    tokio::fs::write(temporary.path().join("plain-file"), "not a package")
        .await
        .unwrap();
    symlink(
        temporary.path().join("host/linked-target"),
        temporary.path().join("linked"),
    )
    .unwrap();
    symlink(
        outside.path().join("outside"),
        temporary.path().join("escape"),
    )
    .unwrap();
    symlink(
        temporary.path().join("missing"),
        temporary.path().join("dangling"),
    )
    .unwrap();
    symlink(
        temporary.path().join("plain-file"),
        temporary.path().join("file-link"),
    )
    .unwrap();
    let source = DirectorySkillSource::new(SkillLayer::Builtin, temporary.path());

    let packages = source.discover().await.unwrap();
    let ids = packages
        .iter()
        .map(|package| package.descriptor.id.as_str())
        .collect::<Vec<_>>();

    assert_eq!(ids, vec!["com.example.host", "com.example.linked"]);
    assert_eq!(
        packages[1].root,
        tokio::fs::canonicalize(temporary.path().join("host/linked-target"))
            .await
            .unwrap()
    );
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

#[tokio::test]
async fn package_tree_hash_distinguishes_nul_delimiter_collision_trees() {
    let single_file = tempfile::tempdir().unwrap();
    let two_files = tempfile::tempdir().unwrap();
    tokio::fs::write(single_file.path().join("a"), b"\0b\0")
        .await
        .unwrap();
    tokio::fs::write(two_files.path().join("a"), b"")
        .await
        .unwrap();
    tokio::fs::write(two_files.path().join("b"), b"")
        .await
        .unwrap();

    assert_ne!(
        hash_package_tree(single_file.path()).await.unwrap(),
        hash_package_tree(two_files.path()).await.unwrap()
    );
}

#[tokio::test]
async fn package_tree_hash_matches_canonical_cross_platform_vector() {
    let temporary = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(temporary.path().join("nested"))
        .await
        .unwrap();
    tokio::fs::write(temporary.path().join("SKILL.md"), b"hello\n")
        .await
        .unwrap();
    tokio::fs::write(
        temporary.path().join("nested/config.json"),
        b"{\"enabled\":true}\n",
    )
    .await
    .unwrap();

    assert_eq!(
        hash_package_tree(temporary.path()).await.unwrap(),
        "91d72d68611bb1df80dd2d5b15290e00019e6892e957a3da6298efec69677bae"
    );
}

#[test]
fn canonical_package_path_normalizes_nfd_components_to_nfc() {
    let nfd = Path::new("cafe\u{301}.txt");

    assert_eq!(
        canonical_relative_path(nfd).unwrap(),
        "caf\u{e9}.txt".as_bytes()
    );
}

#[test]
fn portable_collision_key_case_folds_the_complete_relative_path() {
    assert_eq!(
        portable_collision_key(Path::new("Root/Nested/A.txt")).unwrap(),
        portable_collision_key(Path::new("root/nested/a.txt")).unwrap()
    );
}

#[test]
fn portable_collision_key_treats_nfc_and_nfd_paths_as_equal() {
    assert_eq!(
        portable_collision_key(Path::new("nested/caf\u{e9}.txt")).unwrap(),
        portable_collision_key(Path::new("nested/cafe\u{301}.txt")).unwrap()
    );
}

#[test]
fn portable_collision_key_full_folds_sigma_and_final_sigma() {
    assert_eq!(
        portable_collision_key(Path::new("nested/\u{3c3}.txt")).unwrap(),
        portable_collision_key(Path::new("nested/\u{3c2}.txt")).unwrap()
    );
}

#[test]
fn portable_collision_key_full_folds_sharp_s_and_ss() {
    assert_eq!(
        portable_collision_key(Path::new("nested/\u{df}.txt")).unwrap(),
        portable_collision_key(Path::new("nested/ss.txt")).unwrap()
    );
}

#[test]
fn portable_collision_key_folds_georgian_mtavruli_to_mkhedruli() {
    assert_eq!(
        portable_collision_key(Path::new("nested/\u{1c90}.txt")).unwrap(),
        portable_collision_key(Path::new("nested/\u{10d0}.txt")).unwrap()
    );
}

#[tokio::test]
async fn package_tree_hash_normalizes_a_single_nfd_path_to_nfc() {
    let nfc_tree = tempfile::tempdir().unwrap();
    let nfd_tree = tempfile::tempdir().unwrap();
    tokio::fs::write(nfc_tree.path().join("caf\u{e9}.txt"), b"content")
        .await
        .unwrap();
    tokio::fs::write(nfd_tree.path().join("cafe\u{301}.txt"), b"content")
        .await
        .unwrap();

    assert_eq!(
        hash_package_tree(nfc_tree.path()).await.unwrap(),
        hash_package_tree(nfd_tree.path()).await.unwrap()
    );
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn package_tree_hash_rejects_ascii_case_file_collisions() {
    let temporary = tempfile::tempdir().unwrap();
    tokio::fs::write(temporary.path().join("A"), b"upper")
        .await
        .unwrap();
    tokio::fs::write(temporary.path().join("a"), b"lower")
        .await
        .unwrap();

    let error = hash_package_tree(temporary.path()).await.unwrap_err();

    assert!(error.to_string().contains("portable path collision"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn package_tree_hash_rejects_nfc_nfd_directory_collisions() {
    let temporary = tempfile::tempdir().unwrap();
    tokio::fs::create_dir(temporary.path().join("caf\u{e9}"))
        .await
        .unwrap();
    tokio::fs::create_dir(temporary.path().join("cafe\u{301}"))
        .await
        .unwrap();

    let error = hash_package_tree(temporary.path()).await.unwrap_err();

    assert!(error.to_string().contains("portable path collision"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn package_tree_hash_rejects_sigma_final_sigma_file_collisions() {
    let temporary = tempfile::tempdir().unwrap();
    tokio::fs::write(temporary.path().join("\u{3c3}.txt"), b"sigma")
        .await
        .unwrap();
    tokio::fs::write(temporary.path().join("\u{3c2}.txt"), b"final sigma")
        .await
        .unwrap();

    let error = hash_package_tree(temporary.path()).await.unwrap_err();

    assert!(error.to_string().contains("portable path collision"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn package_tree_hash_rejects_sharp_s_ss_file_collisions() {
    let temporary = tempfile::tempdir().unwrap();
    tokio::fs::write(temporary.path().join("\u{df}.txt"), b"sharp s")
        .await
        .unwrap();
    tokio::fs::write(temporary.path().join("ss.txt"), b"ss")
        .await
        .unwrap();

    let error = hash_package_tree(temporary.path()).await.unwrap_err();

    assert!(error.to_string().contains("portable path collision"));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn package_tree_hash_rejects_georgian_case_file_collisions() {
    let temporary = tempfile::tempdir().unwrap();
    tokio::fs::write(temporary.path().join("\u{1c90}.txt"), b"mtavruli")
        .await
        .unwrap();
    tokio::fs::write(temporary.path().join("\u{10d0}.txt"), b"mkhedruli")
        .await
        .unwrap();

    let error = hash_package_tree(temporary.path()).await.unwrap_err();

    assert!(error.to_string().contains("portable path collision"));
}

#[cfg(unix)]
#[tokio::test]
async fn package_tree_hash_rejects_non_utf8_path_components() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let name = OsString::from_vec(vec![b'n', b'o', b'n', b'-', 0xff]);

    let error = canonical_relative_path(Path::new(&name)).unwrap_err();

    assert!(error.to_string().contains("UTF-8"));
}

#[cfg(unix)]
#[tokio::test]
async fn package_tree_hash_rejects_backslash_path_components() {
    let temporary = tempfile::tempdir().unwrap();
    tokio::fs::write(temporary.path().join("not\\portable"), b"content")
        .await
        .unwrap();

    let error = hash_package_tree(temporary.path()).await.unwrap_err();

    assert!(error.to_string().contains("backslash"));
}

#[cfg(unix)]
#[tokio::test]
async fn package_tree_hash_rejects_special_files() {
    use std::os::unix::net::UnixListener;

    let temporary = tempfile::tempdir().unwrap();
    let _listener = UnixListener::bind(temporary.path().join("runtime.sock")).unwrap();

    let error = hash_package_tree(temporary.path()).await.unwrap_err();

    assert!(error.to_string().contains("special files"));
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
        package_root.join("agentweave.json"),
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
