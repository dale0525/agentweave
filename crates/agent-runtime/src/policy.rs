use crate::tools::ToolPermission;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Never,
    OnWorkspaceWrite,
    OnCommand,
    OnWrites,
    OnSensitiveOperations,
}

impl ApprovalPolicy {
    pub fn requires_approval(self, permission: ToolPermission) -> bool {
        if permission == ToolPermission::ManageSkills {
            return false;
        }
        match self {
            Self::Never => false,
            Self::OnWorkspaceWrite => matches!(
                permission,
                ToolPermission::WriteWorkspace | ToolPermission::ExecuteCommand
            ),
            Self::OnCommand => permission == ToolPermission::ExecuteCommand,
            Self::OnWrites => matches!(
                permission,
                ToolPermission::WriteWorkspace
                    | ToolPermission::ExecuteCommand
                    | ToolPermission::PersistData
                    | ToolPermission::ExternalWrite
                    | ToolPermission::DestructiveWrite
            ),
            Self::OnSensitiveOperations => permission != ToolPermission::ReadWorkspace,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum FilesystemPolicy {
    WorkspaceOnly,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum CommandPolicy {
    Disabled,
    DevelopmentOnly,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum NetworkPolicy {
    UnrestrictedPlaceholder,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct SandboxProfile {
    pub filesystem: FilesystemPolicy,
    pub command: CommandPolicy,
    pub network: NetworkPolicy,
}

impl Default for SandboxProfile {
    fn default() -> Self {
        Self {
            filesystem: FilesystemPolicy::WorkspaceOnly,
            command: CommandPolicy::DevelopmentOnly,
            network: NetworkPolicy::UnrestrictedPlaceholder,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approval_policy_identifies_permissions_that_require_approval() {
        let cases = [
            (ApprovalPolicy::Never, ToolPermission::ReadWorkspace, false),
            (ApprovalPolicy::Never, ToolPermission::WriteWorkspace, false),
            (ApprovalPolicy::Never, ToolPermission::ExecuteCommand, false),
            (ApprovalPolicy::Never, ToolPermission::ManageSkills, false),
            (
                ApprovalPolicy::OnWorkspaceWrite,
                ToolPermission::ReadWorkspace,
                false,
            ),
            (
                ApprovalPolicy::OnWorkspaceWrite,
                ToolPermission::WriteWorkspace,
                true,
            ),
            (
                ApprovalPolicy::OnWorkspaceWrite,
                ToolPermission::ExecuteCommand,
                true,
            ),
            (
                ApprovalPolicy::OnWorkspaceWrite,
                ToolPermission::ManageSkills,
                false,
            ),
            (
                ApprovalPolicy::OnCommand,
                ToolPermission::ReadWorkspace,
                false,
            ),
            (
                ApprovalPolicy::OnCommand,
                ToolPermission::WriteWorkspace,
                false,
            ),
            (
                ApprovalPolicy::OnCommand,
                ToolPermission::ExecuteCommand,
                true,
            ),
            (
                ApprovalPolicy::OnCommand,
                ToolPermission::ManageSkills,
                false,
            ),
        ];

        for (policy, permission, expected) in cases {
            assert_eq!(policy.requires_approval(permission), expected);
        }
    }

    #[test]
    fn generic_approval_policy_never_approves_skill_management() {
        for policy in [
            ApprovalPolicy::Never,
            ApprovalPolicy::OnWorkspaceWrite,
            ApprovalPolicy::OnCommand,
            ApprovalPolicy::OnWrites,
            ApprovalPolicy::OnSensitiveOperations,
        ] {
            assert!(!policy.requires_approval(ToolPermission::ManageSkills));
        }
    }

    #[test]
    fn default_sandbox_profile_is_explicit_about_network_placeholder() {
        let profile = SandboxProfile::default();

        assert_eq!(profile.filesystem, FilesystemPolicy::WorkspaceOnly);
        assert_eq!(profile.command, CommandPolicy::DevelopmentOnly);
        assert_eq!(profile.network, NetworkPolicy::UnrestrictedPlaceholder);
    }
}
