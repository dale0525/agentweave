use crate::skill_package::{SkillPackageId, SkillPackageKind};
use crate::skill_policy::{
    ActorContext, SkillGrant, SkillManagementMode, SkillManagementPolicy, SkillOperation,
};

#[test]
fn disabled_policy_denies_every_management_operation_including_inspect() {
    let policy = SkillManagementPolicy::default();
    let actor = ActorContext::owner("owner-1", [SkillGrant::Inspect, SkillGrant::Activate]);

    assert!(!policy.allows(
        &actor,
        SkillOperation::Inspect,
        SkillPackageKind::InstructionOnly
    ));
    assert!(!policy.allows(
        &actor,
        SkillOperation::Activate,
        SkillPackageKind::InstructionOnly
    ));
}

#[test]
fn diagnostics_only_requires_inspect_grant_and_denies_mutation() {
    let policy = SkillManagementPolicy {
        mode: SkillManagementMode::DiagnosticsOnly,
        ..SkillManagementPolicy::default()
    };
    let inspector = ActorContext::anonymous().with_grants([SkillGrant::Inspect]);
    let mutator = ActorContext::anonymous().with_grants([SkillGrant::CreateDraft]);

    assert!(policy.allows(
        &inspector,
        SkillOperation::Inspect,
        SkillPackageKind::NativeRuntime
    ));
    assert!(!policy.allows(
        &ActorContext::anonymous(),
        SkillOperation::Inspect,
        SkillPackageKind::InstructionOnly
    ));
    assert!(!policy.allows(
        &mutator,
        SkillOperation::CreateDraft,
        SkillPackageKind::InstructionOnly
    ));
}

#[test]
fn organization_managed_only_exposes_explicitly_granted_inspection() {
    let policy = SkillManagementPolicy {
        mode: SkillManagementMode::OrganizationManaged,
        ..SkillManagementPolicy::default()
    };
    let actor = ActorContext::owner("owner-1", [SkillGrant::Inspect, SkillGrant::Activate]);

    assert!(policy.allows(
        &actor,
        SkillOperation::Inspect,
        SkillPackageKind::HostToolsOnly
    ));
    assert!(!policy.allows(
        &actor,
        SkillOperation::Activate,
        SkillPackageKind::HostToolsOnly
    ));
}

#[test]
fn owner_only_requires_owner_role_explicit_grant_and_allowed_kind() {
    let policy = SkillManagementPolicy::owner_only();
    let owner = ActorContext::owner("owner-1", [SkillGrant::Activate]);
    let non_owner = ActorContext::anonymous().with_grants([SkillGrant::Activate]);
    let owner_without_grant = ActorContext::owner("owner-2", []);

    assert!(policy.allows(
        &owner,
        SkillOperation::Activate,
        SkillPackageKind::InstructionOnly
    ));
    assert!(!policy.allows(
        &non_owner,
        SkillOperation::Activate,
        SkillPackageKind::InstructionOnly
    ));
    assert!(!policy.allows(
        &owner_without_grant,
        SkillOperation::Activate,
        SkillPackageKind::InstructionOnly
    ));
    assert!(!policy.allows(
        &owner,
        SkillOperation::Activate,
        SkillPackageKind::NativeRuntime
    ));
}

#[test]
fn owner_only_authoring_requires_agent_authoring_gate() {
    let actor = ActorContext::owner("owner-1", [SkillGrant::CreateDraft, SkillGrant::EditDraft]);
    let mut policy = SkillManagementPolicy::owner_only();
    policy.agent_authoring = false;

    assert!(!policy.allows(
        &actor,
        SkillOperation::CreateDraft,
        SkillPackageKind::InstructionOnly
    ));
    assert!(!policy.allows(
        &actor,
        SkillOperation::EditDraft,
        SkillPackageKind::InstructionOnly
    ));
}

#[test]
fn owner_grant_does_not_allow_native_runtime_when_kind_is_blocked() {
    let policy = SkillManagementPolicy::owner_only();
    let actor = ActorContext::owner("owner-1", [SkillGrant::CreateDraft, SkillGrant::Activate]);

    assert!(policy.allows(
        &actor,
        SkillOperation::CreateDraft,
        SkillPackageKind::InstructionOnly
    ));
    assert!(!policy.allows(
        &actor,
        SkillOperation::CreateDraft,
        SkillPackageKind::NativeRuntime
    ));
}

#[test]
fn protected_override_requires_allowlist_and_override_grant() {
    let package = SkillPackageId::parse("generalagent.core.runtime").unwrap();
    let policy = SkillManagementPolicy::owner_only()
        .protect(package.clone())
        .allow_override(package.clone());

    assert!(policy.can_override(
        &ActorContext::owner("owner-1", [SkillGrant::OverrideBuiltin]),
        &package
    ));
    assert!(!policy.can_override(&ActorContext::owner("owner-1", []), &package));
}

#[test]
fn protected_override_denies_packages_missing_from_allowlist() {
    let package = SkillPackageId::parse("generalagent.core.runtime").unwrap();
    let policy = SkillManagementPolicy::owner_only().protect(package.clone());
    let actor = ActorContext::owner("owner-1", [SkillGrant::OverrideBuiltin]);

    assert!(!policy.can_override(&actor, &package));
}

#[test]
fn override_denies_unprotected_packages_and_non_owner_actors() {
    let package = SkillPackageId::parse("generalagent.core.runtime").unwrap();
    let policy = SkillManagementPolicy::owner_only().allow_override(package.clone());
    let non_owner = ActorContext::anonymous().with_grants([SkillGrant::OverrideBuiltin]);

    assert!(!policy.can_override(
        &ActorContext::owner("owner-1", [SkillGrant::OverrideBuiltin]),
        &package
    ));
    assert!(!policy.can_override(&non_owner, &package));
}

#[test]
fn operation_names_and_grants_have_a_stable_mapping() {
    let cases = [
        (SkillOperation::Inspect, "inspect", SkillGrant::Inspect),
        (
            SkillOperation::CreateDraft,
            "create_draft",
            SkillGrant::CreateDraft,
        ),
        (
            SkillOperation::EditDraft,
            "edit_draft",
            SkillGrant::EditDraft,
        ),
        (SkillOperation::Validate, "validate", SkillGrant::Validate),
        (SkillOperation::Test, "test", SkillGrant::Test),
        (SkillOperation::Activate, "activate", SkillGrant::Activate),
        (SkillOperation::Disable, "disable", SkillGrant::Disable),
        (
            SkillOperation::DeleteManaged,
            "delete_managed",
            SkillGrant::DeleteManaged,
        ),
        (SkillOperation::Import, "import", SkillGrant::Import),
        (SkillOperation::Export, "export", SkillGrant::Export),
        (SkillOperation::Rollback, "rollback", SkillGrant::Rollback),
        (
            SkillOperation::OverrideBuiltin,
            "override_builtin",
            SkillGrant::OverrideBuiltin,
        ),
    ];

    for (operation, name, grant) in cases {
        assert_eq!(operation.as_str(), name);
        assert_eq!(operation.required_grant(), grant);
    }
}
