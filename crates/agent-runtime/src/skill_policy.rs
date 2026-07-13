use crate::skill_package::{SkillPackageId, SkillPackageKind};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillManagementMode {
    Disabled,
    DiagnosticsOnly,
    OwnerOnly,
    OrganizationManaged,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum SkillGrant {
    Inspect,
    CreateDraft,
    EditDraft,
    Validate,
    Test,
    Activate,
    Disable,
    DeleteManaged,
    Import,
    Export,
    Rollback,
    OverrideBuiltin,
    GrantPermissions,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkillOperation {
    Inspect,
    CreateDraft,
    EditDraft,
    Validate,
    Test,
    Activate,
    Disable,
    DeleteManaged,
    Import,
    Export,
    Rollback,
    OverrideBuiltin,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct ActorContext {
    pub actor_id: String,
    pub role: String,
    pub tenant_id: Option<String>,
    pub device_id: Option<String>,
    pub grants: BTreeSet<SkillGrant>,
}

impl ActorContext {
    pub fn anonymous() -> Self {
        Self {
            actor_id: "anonymous".into(),
            role: "user".into(),
            tenant_id: None,
            device_id: None,
            grants: BTreeSet::new(),
        }
    }

    pub fn owner(id: impl Into<String>, grants: impl IntoIterator<Item = SkillGrant>) -> Self {
        Self {
            actor_id: id.into(),
            role: "owner".into(),
            tenant_id: None,
            device_id: None,
            grants: grants.into_iter().collect(),
        }
    }

    pub fn with_grants(mut self, grants: impl IntoIterator<Item = SkillGrant>) -> Self {
        self.grants = grants.into_iter().collect();
        self
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct SkillManagementPolicy {
    pub mode: SkillManagementMode,
    pub agent_authoring: bool,
    pub allowed_kinds: BTreeSet<SkillPackageKind>,
    pub protected_packages: BTreeSet<SkillPackageId>,
    pub allowed_overrides: BTreeSet<SkillPackageId>,
    #[serde(default)]
    pub rollback_approval_required: bool,
}

impl Default for SkillManagementPolicy {
    fn default() -> Self {
        Self {
            mode: SkillManagementMode::Disabled,
            agent_authoring: false,
            allowed_kinds: BTreeSet::new(),
            protected_packages: BTreeSet::new(),
            allowed_overrides: BTreeSet::new(),
            rollback_approval_required: false,
        }
    }
}

impl SkillManagementPolicy {
    pub fn owner_only() -> Self {
        Self {
            mode: SkillManagementMode::OwnerOnly,
            agent_authoring: true,
            allowed_kinds: [
                SkillPackageKind::InstructionOnly,
                SkillPackageKind::HostToolsOnly,
            ]
            .into_iter()
            .collect(),
            ..Self::default()
        }
    }

    pub fn protect(mut self, package_id: SkillPackageId) -> Self {
        self.protected_packages.insert(package_id);
        self
    }

    pub fn allow_override(mut self, package_id: SkillPackageId) -> Self {
        self.allowed_overrides.insert(package_id);
        self
    }

    pub fn can_inspect(&self, actor: &ActorContext) -> bool {
        match self.mode {
            SkillManagementMode::Disabled => false,
            SkillManagementMode::DiagnosticsOnly | SkillManagementMode::OrganizationManaged => {
                actor.grants.contains(&SkillGrant::Inspect)
            }
            SkillManagementMode::OwnerOnly => {
                actor.role == "owner" && actor.grants.contains(&SkillGrant::Inspect)
            }
        }
    }

    pub fn allows(
        &self,
        actor: &ActorContext,
        operation: SkillOperation,
        kind: SkillPackageKind,
    ) -> bool {
        if operation == SkillOperation::OverrideBuiltin {
            return false;
        }

        match self.mode {
            SkillManagementMode::Disabled => false,
            SkillManagementMode::DiagnosticsOnly | SkillManagementMode::OrganizationManaged => {
                operation == SkillOperation::Inspect && actor.grants.contains(&SkillGrant::Inspect)
            }
            SkillManagementMode::OwnerOnly => {
                actor.role == "owner"
                    && self.allowed_kinds.contains(&kind)
                    && actor.grants.contains(&operation.required_grant())
                    && (!operation.is_authoring() || self.agent_authoring)
            }
        }
    }

    pub fn can_override(&self, actor: &ActorContext, id: &SkillPackageId) -> bool {
        self.mode == SkillManagementMode::OwnerOnly
            && actor.role == "owner"
            && !self.protected_packages.contains(id)
            && self.allowed_overrides.contains(id)
            && actor.grants.contains(&SkillGrant::OverrideBuiltin)
    }

    pub fn can_author_conversationally(&self, actor: &ActorContext) -> bool {
        self.mode == SkillManagementMode::OwnerOnly
            && self.agent_authoring
            && actor.role == "owner"
            && self.allowed_kinds.iter().copied().any(|kind| {
                [
                    SkillOperation::CreateDraft,
                    SkillOperation::EditDraft,
                    SkillOperation::Validate,
                    SkillOperation::Test,
                ]
                .into_iter()
                .any(|operation| self.allows(actor, operation, kind))
            })
    }
}

impl SkillOperation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inspect => "inspect",
            Self::CreateDraft => "create_draft",
            Self::EditDraft => "edit_draft",
            Self::Validate => "validate",
            Self::Test => "test",
            Self::Activate => "activate",
            Self::Disable => "disable",
            Self::DeleteManaged => "delete_managed",
            Self::Import => "import",
            Self::Export => "export",
            Self::Rollback => "rollback",
            Self::OverrideBuiltin => "override_builtin",
        }
    }

    pub fn required_grant(self) -> SkillGrant {
        match self {
            Self::Inspect => SkillGrant::Inspect,
            Self::CreateDraft => SkillGrant::CreateDraft,
            Self::EditDraft => SkillGrant::EditDraft,
            Self::Validate => SkillGrant::Validate,
            Self::Test => SkillGrant::Test,
            Self::Activate => SkillGrant::Activate,
            Self::Disable => SkillGrant::Disable,
            Self::DeleteManaged => SkillGrant::DeleteManaged,
            Self::Import => SkillGrant::Import,
            Self::Export => SkillGrant::Export,
            Self::Rollback => SkillGrant::Rollback,
            Self::OverrideBuiltin => SkillGrant::OverrideBuiltin,
        }
    }

    fn is_authoring(self) -> bool {
        match self {
            Self::Inspect => false,
            Self::CreateDraft => true,
            Self::EditDraft => true,
            Self::Validate => false,
            Self::Test => false,
            Self::Activate => false,
            Self::Disable => false,
            Self::DeleteManaged => false,
            Self::Import => false,
            Self::Export => false,
            Self::Rollback => false,
            Self::OverrideBuiltin => false,
        }
    }
}
