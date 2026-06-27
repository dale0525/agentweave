use crate::tools::ToolPermission;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum ApprovalPolicy {
    Never,
    OnWorkspaceWrite,
    OnCommand,
}

impl ApprovalPolicy {
    pub fn requires_approval(self, permission: ToolPermission) -> bool {
        match self {
            Self::Never => false,
            Self::OnWorkspaceWrite => {
                matches!(
                    permission,
                    ToolPermission::WriteWorkspace | ToolPermission::ExecuteCommand
                )
            }
            Self::OnCommand => permission == ToolPermission::ExecuteCommand,
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
        assert!(!ApprovalPolicy::Never.requires_approval(ToolPermission::WriteWorkspace));
        assert!(ApprovalPolicy::OnWorkspaceWrite.requires_approval(ToolPermission::WriteWorkspace));
        assert!(ApprovalPolicy::OnWorkspaceWrite.requires_approval(ToolPermission::ExecuteCommand));
        assert!(!ApprovalPolicy::OnCommand.requires_approval(ToolPermission::WriteWorkspace));
        assert!(ApprovalPolicy::OnCommand.requires_approval(ToolPermission::ExecuteCommand));
    }

    #[test]
    fn default_sandbox_profile_is_explicit_about_network_placeholder() {
        let profile = SandboxProfile::default();

        assert_eq!(profile.filesystem, FilesystemPolicy::WorkspaceOnly);
        assert_eq!(profile.command, CommandPolicy::DevelopmentOnly);
        assert_eq!(profile.network, NetworkPolicy::UnrestrictedPlaceholder);
    }
}
