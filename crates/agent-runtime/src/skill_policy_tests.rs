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
fn generic_allows_never_authorizes_builtin_override() {
    let package = SkillPackageId::parse("generalagent.core.runtime").unwrap();
    let policy = SkillManagementPolicy::owner_only()
        .protect(package.clone())
        .allow_override(package.clone());
    let actor = ActorContext::owner("owner-1", [SkillGrant::OverrideBuiltin]);

    assert!(!policy.allows(
        &actor,
        SkillOperation::OverrideBuiltin,
        SkillPackageKind::InstructionOnly
    ));
    assert!(policy.can_override(&actor, &package));
}

#[test]
fn inspect_authorization_is_package_independent() {
    let diagnostics = SkillManagementPolicy {
        mode: SkillManagementMode::DiagnosticsOnly,
        ..SkillManagementPolicy::default()
    };
    let inspector = ActorContext::anonymous().with_grants([SkillGrant::Inspect]);

    assert!(diagnostics.can_inspect(&inspector));
    assert!(!diagnostics.can_inspect(&ActorContext::anonymous()));
    assert!(!SkillManagementPolicy::default().can_inspect(&inspector));
}

#[test]
fn protected_override_denies_packages_missing_from_allowlist() {
    let package = SkillPackageId::parse("generalagent.core.runtime").unwrap();
    let policy = SkillManagementPolicy::owner_only().protect(package.clone());
    let actor = ActorContext::owner("owner-1", [SkillGrant::OverrideBuiltin]);

    assert!(!policy.can_override(&actor, &package));
}

#[test]
fn allowlisted_unprotected_override_accepts_owner_grant_and_denies_non_owner() {
    let package = SkillPackageId::parse("generalagent.core.runtime").unwrap();
    let policy = SkillManagementPolicy::owner_only().allow_override(package.clone());
    let non_owner = ActorContext::anonymous().with_grants([SkillGrant::OverrideBuiltin]);

    assert!(policy.can_override(
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

#[test]
fn actor_context_grants_serialize_deterministically_and_round_trip() {
    let actor = ActorContext {
        actor_id: "owner-1".into(),
        role: "owner".into(),
        tenant_id: Some("tenant-1".into()),
        device_id: Some("device-1".into()),
        grants: [
            SkillGrant::Rollback,
            SkillGrant::Inspect,
            SkillGrant::Activate,
            SkillGrant::Inspect,
        ]
        .into_iter()
        .collect(),
    };
    let expected = serde_json::json!({
        "actor_id": "owner-1",
        "role": "owner",
        "tenant_id": "tenant-1",
        "device_id": "device-1",
        "grants": ["inspect", "activate", "rollback"]
    });

    let serialized = serde_json::to_value(&actor).unwrap();
    assert_eq!(serialized, expected);

    let decoded: ActorContext = serde_json::from_value(serialized).unwrap();
    assert_eq!(decoded, actor);
    assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
}

#[test]
fn skill_management_policy_serializes_deterministically_and_round_trips() {
    let protected_alpha = SkillPackageId::parse("generalagent.core.alpha").unwrap();
    let protected_runtime = SkillPackageId::parse("generalagent.core.runtime").unwrap();
    let policy = SkillManagementPolicy {
        mode: SkillManagementMode::OwnerOnly,
        agent_authoring: false,
        allowed_kinds: [
            SkillPackageKind::NativeRuntime,
            SkillPackageKind::InstructionOnly,
        ]
        .into_iter()
        .collect(),
        protected_packages: [protected_runtime.clone(), protected_alpha]
            .into_iter()
            .collect(),
        allowed_overrides: [protected_runtime].into_iter().collect(),
        activation_approval_required: false,
        permission_escalation_approval_required: true,
        rollback_approval_required: false,
    };
    let expected = serde_json::json!({
        "mode": "owner_only",
        "agent_authoring": false,
        "allowed_kinds": ["instruction_only", "native_runtime"],
        "protected_packages": ["generalagent.core.alpha", "generalagent.core.runtime"],
        "allowed_overrides": ["generalagent.core.runtime"],
        "activation_approval_required": false,
        "permission_escalation_approval_required": true,
        "rollback_approval_required": false
    });

    let serialized = serde_json::to_value(&policy).unwrap();
    assert_eq!(serialized, expected);

    let decoded: SkillManagementPolicy = serde_json::from_value(serialized).unwrap();
    assert_eq!(decoded, policy);
    assert_eq!(serde_json::to_value(decoded).unwrap(), expected);
}

#[test]
fn all_skill_operations_have_stable_snake_case_serde() {
    let cases = [
        (SkillOperation::Inspect, "inspect"),
        (SkillOperation::CreateDraft, "create_draft"),
        (SkillOperation::EditDraft, "edit_draft"),
        (SkillOperation::Validate, "validate"),
        (SkillOperation::Test, "test"),
        (SkillOperation::Activate, "activate"),
        (SkillOperation::Disable, "disable"),
        (SkillOperation::DeleteManaged, "delete_managed"),
        (SkillOperation::Import, "import"),
        (SkillOperation::Export, "export"),
        (SkillOperation::Rollback, "rollback"),
        (SkillOperation::OverrideBuiltin, "override_builtin"),
    ];

    for (operation, value) in cases {
        let serialized = serde_json::to_value(operation).unwrap();
        assert_eq!(serialized, serde_json::json!(value));
        assert_eq!(
            serde_json::from_value::<SkillOperation>(serialized).unwrap(),
            operation
        );
    }
}
