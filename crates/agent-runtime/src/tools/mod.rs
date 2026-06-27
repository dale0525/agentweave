pub mod path;
pub mod result;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

const DEFAULT_MAX_TOOL_CALLS_PER_TURN: usize = 16;
const DEFAULT_TOOL_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum CommandMode {
    Disabled,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub mode: RuntimeMode,
    pub command_mode: CommandMode,
    pub max_tool_calls_per_turn: usize,
    pub tool_timeout_ms: u64,
    pub output_limit_bytes: usize,
}

impl RuntimeConfig {
    pub fn workspace_write(workspace_root: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, cwd, RuntimeMode::WorkspaceWrite)
    }

    pub fn read_only(workspace_root: impl Into<PathBuf>, cwd: impl Into<PathBuf>) -> Self {
        Self::new(workspace_root, cwd, RuntimeMode::ReadOnly)
    }

    fn new(workspace_root: impl Into<PathBuf>, cwd: impl Into<PathBuf>, mode: RuntimeMode) -> Self {
        Self {
            workspace_root: workspace_root.into(),
            cwd: cwd.into(),
            mode,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: DEFAULT_MAX_TOOL_CALLS_PER_TURN,
            tool_timeout_ms: DEFAULT_TOOL_TIMEOUT_MS,
            output_limit_bytes: DEFAULT_OUTPUT_LIMIT_BYTES,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum RuntimeMode {
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum ToolPermission {
    ReadWorkspace,
    WriteWorkspace,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    pub permission: ToolPermission,
}

pub fn permission_allowed(mode: RuntimeMode, permission: ToolPermission) -> bool {
    match (mode, permission) {
        (RuntimeMode::ReadOnly, ToolPermission::WriteWorkspace) => false,
        (RuntimeMode::ReadOnly, ToolPermission::ReadWorkspace)
        | (RuntimeMode::WorkspaceWrite, ToolPermission::ReadWorkspace)
        | (RuntimeMode::WorkspaceWrite, ToolPermission::WriteWorkspace) => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn read_only_blocks_workspace_writes() {
        assert!(permission_allowed(
            RuntimeMode::ReadOnly,
            ToolPermission::ReadWorkspace
        ));
        assert!(!permission_allowed(
            RuntimeMode::ReadOnly,
            ToolPermission::WriteWorkspace
        ));
        assert!(permission_allowed(
            RuntimeMode::WorkspaceWrite,
            ToolPermission::ReadWorkspace
        ));
        assert!(permission_allowed(
            RuntimeMode::WorkspaceWrite,
            ToolPermission::WriteWorkspace
        ));
    }

    #[test]
    fn runtime_config_defaults_to_command_disabled() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("project");
        let workspace_write = RuntimeConfig::workspace_write(workspace_root.clone(), cwd.clone());
        let read_only = RuntimeConfig::read_only(workspace_root.clone(), cwd.clone());

        assert_eq!(workspace_write.workspace_root, workspace_root);
        assert_eq!(workspace_write.cwd, cwd);
        assert_eq!(workspace_write.mode, RuntimeMode::WorkspaceWrite);
        assert_eq!(workspace_write.command_mode, CommandMode::Disabled);
        assert_eq!(workspace_write.max_tool_calls_per_turn, 16);
        assert_eq!(workspace_write.tool_timeout_ms, 30_000);
        assert_eq!(workspace_write.output_limit_bytes, 64 * 1024);

        assert_eq!(read_only.mode, RuntimeMode::ReadOnly);
        assert_eq!(read_only.command_mode, CommandMode::Disabled);
        assert_eq!(read_only.max_tool_calls_per_turn, 16);
        assert_eq!(read_only.tool_timeout_ms, 30_000);
        assert_eq!(read_only.output_limit_bytes, 64 * 1024);
    }
}
