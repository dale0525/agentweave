pub mod builtin;
pub mod path;
pub mod result;

use crate::skill::SkillRegistry;
use builtin::BuiltInTools;
use result::{ToolError, ToolResult, ToolResultMetadata};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;
use std::time::{Duration, Instant};

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

#[derive(Debug)]
pub struct ToolRegistry {
    builtins: BuiltInTools,
    skills: SkillRegistry,
    tool_timeout: Duration,
    output_limit_bytes: usize,
}

impl ToolRegistry {
    pub fn new(skills: SkillRegistry, config: &RuntimeConfig) -> Self {
        Self {
            builtins: BuiltInTools::new(config.clone()),
            skills,
            tool_timeout: Duration::from_millis(config.tool_timeout_ms),
            output_limit_bytes: config.output_limit_bytes,
        }
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self.builtins.definitions();
        definitions.extend(self.skills.tools().into_iter().map(|tool| ToolDefinition {
            name: tool.name,
            description: tool.description,
            input_schema: tool.input_schema,
            permission: ToolPermission::ReadWorkspace,
        }));
        definitions
    }

    pub async fn execute(&self, name: &str, call_id: &str, arguments: Value) -> ToolResult {
        let started = Instant::now();
        let execution = tokio::time::timeout(
            self.tool_timeout,
            self.execute_without_timeout(name, call_id, arguments, started),
        );

        match execution.await {
            Ok(result) => self.apply_output_limit(result),
            Err(_) => registry_failure(
                name,
                call_id,
                "timeout",
                "tool execution timed out",
                true,
                registry_metadata(started),
            ),
        }
    }

    async fn execute_without_timeout(
        &self,
        name: &str,
        call_id: &str,
        arguments: Value,
        started: Instant,
    ) -> ToolResult {
        if BuiltInTools::handles(name) {
            return self.builtins.execute(name, call_id, arguments).await;
        }

        if !self
            .skills
            .tools()
            .iter()
            .any(|tool| tool.name.as_str() == name)
        {
            return registry_failure(
                name,
                call_id,
                "unknown_tool",
                format!("unknown tool: {name}"),
                false,
                ToolResultMetadata::default(),
            );
        }

        match self
            .skills
            .execute_with_output_limit(name, arguments, self.output_limit_bytes)
            .await
        {
            Ok(value) => ToolResult::success(name, call_id, value, registry_metadata(started)),
            Err(error) => {
                let message = error.to_string();
                let code = skill_error_code(&message);
                let mut metadata = registry_metadata(started);
                if code == "output_limit_exceeded" {
                    metadata.output_truncated = true;
                }
                registry_failure(name, call_id, code, message, false, metadata)
            }
        }
    }

    fn apply_output_limit(&self, result: ToolResult) -> ToolResult {
        if !result.ok {
            return result;
        }

        let data_exceeds_limit = result
            .data
            .as_ref()
            .map(|data| serialized_len(data) > self.output_limit_bytes)
            .unwrap_or(false);
        let result_exceeds_limit = serialized_len(&result) > self.output_limit_bytes;
        if data_exceeds_limit || result_exceeds_limit {
            let mut metadata = result.metadata;
            metadata.output_truncated = true;
            return registry_failure(
                &result.tool,
                &result.call_id,
                "output_limit_exceeded",
                "tool output exceeded runtime output limit",
                false,
                metadata,
            );
        }

        result
    }
}

fn serialized_len<T: Serialize>(value: &T) -> usize {
    serde_json::to_vec(value)
        .map(|bytes| bytes.len())
        .unwrap_or(usize::MAX)
}

fn registry_metadata(started: Instant) -> ToolResultMetadata {
    ToolResultMetadata {
        duration_ms: started.elapsed().as_millis() as u64,
        ..ToolResultMetadata::default()
    }
}

fn registry_failure(
    tool: &str,
    call_id: &str,
    code: &str,
    message: impl Into<String>,
    retryable: bool,
    metadata: ToolResultMetadata,
) -> ToolResult {
    ToolResult::failure(
        tool,
        call_id,
        ToolError {
            code: code.to_string(),
            message: message.into(),
            retryable,
        },
        metadata,
    )
}

fn skill_error_code(message: &str) -> &'static str {
    if message.contains("unknown tool") {
        "unknown_tool"
    } else if message.contains("Permission denied") {
        "permission_denied"
    } else if message.contains("output limit") {
        "output_limit_exceeded"
    } else {
        "internal_error"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skill::SkillRegistry;
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

    #[tokio::test]
    async fn tool_registry_applies_skill_output_limit() {
        let root = unique_test_dir("registry-output-limit");
        write_skill(
            &root,
            "large",
            "large_output",
            "process.stdin.resume();\nprocess.stdin.on('end', () => process.stdout.write(JSON.stringify({ text: 'abcdef' })));\n",
        )
        .await;
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig {
            workspace_root: root.clone(),
            cwd: root.clone(),
            mode: RuntimeMode::WorkspaceWrite,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 30_000,
            output_limit_bytes: 4,
        };
        let registry = ToolRegistry::new(skills, &config);

        let result = registry
            .execute("large_output", "call-1", serde_json::json!({}))
            .await;

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
        assert!(result.metadata.output_truncated);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn tool_registry_limits_skill_stdout_before_json_parsing() {
        let root = unique_test_dir("registry-skill-stdout-limit");
        write_skill(
            &root,
            "large-stdout",
            "large_stdout",
            "process.stdin.resume();\nprocess.stdin.on('end', () => process.stdout.write('x'.repeat(1024)));\n",
        )
        .await;
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig {
            workspace_root: root.clone(),
            cwd: root.clone(),
            mode: RuntimeMode::WorkspaceWrite,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 30_000,
            output_limit_bytes: 4,
        };
        let registry = ToolRegistry::new(skills, &config);

        let result = registry
            .execute("large_stdout", "call-1", serde_json::json!({}))
            .await;

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
        assert!(result.metadata.output_truncated);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn tool_registry_applies_builtin_output_limit() {
        let root = unique_test_dir("registry-builtin-output-limit");
        tokio::fs::create_dir_all(&root).await.unwrap();
        tokio::fs::write(root.join("big.txt"), "abcdef")
            .await
            .unwrap();
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig {
            workspace_root: root.clone(),
            cwd: root.clone(),
            mode: RuntimeMode::WorkspaceWrite,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 30_000,
            output_limit_bytes: 4,
        };
        let registry = ToolRegistry::new(skills, &config);

        let result = registry
            .execute(
                "read_text_file",
                "call-1",
                serde_json::json!({ "path": "big.txt" }),
            )
            .await;

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
        assert!(result.metadata.output_truncated);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn tool_registry_applies_skill_timeout() {
        let root = unique_test_dir("registry-timeout");
        write_skill(
            &root,
            "slow",
            "slow_output",
            "const fs = require('fs');\nsetTimeout(() => fs.writeFileSync('timed-out.txt', 'late'), 100);\nprocess.stdin.resume();\n",
        )
        .await;
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig {
            workspace_root: root.clone(),
            cwd: root.clone(),
            mode: RuntimeMode::WorkspaceWrite,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 5,
            output_limit_bytes: 64 * 1024,
        };
        let registry = ToolRegistry::new(skills, &config);

        let result = registry
            .execute("slow_output", "call-1", serde_json::json!({}))
            .await;

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "timeout");
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(
            !root.join("slow").join("timed-out.txt").exists(),
            "timed-out skill process should be stopped before it can write after timeout"
        );
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn tool_registry_stops_skill_when_stdout_exceeds_limit() {
        let root = unique_test_dir("registry-stdout-limit");
        write_skill(
            &root,
            "chatty",
            "chatty_output",
            "process.stdin.resume();\nprocess.stdin.on('end', () => {\n  process.stdout.write('x'.repeat(1024));\n  setTimeout(() => {}, 1000);\n});\n",
        )
        .await;
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig {
            workspace_root: root.clone(),
            cwd: root.clone(),
            mode: RuntimeMode::WorkspaceWrite,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 30_000,
            output_limit_bytes: 32,
        };
        let registry = ToolRegistry::new(skills, &config);

        let result = registry
            .execute("chatty_output", "call-1", serde_json::json!({}))
            .await;

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
        assert!(result.metadata.output_truncated);
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn tool_registry_stops_skill_when_stderr_exceeds_limit() {
        let root = unique_test_dir("registry-stderr-limit");
        write_skill(
            &root,
            "chatty",
            "chatty_error",
            "process.stdin.resume();\nprocess.stdin.on('end', () => {\n  process.stderr.write('x'.repeat(1024));\n  setTimeout(() => {}, 1000);\n});\n",
        )
        .await;
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig {
            workspace_root: root.clone(),
            cwd: root.clone(),
            mode: RuntimeMode::WorkspaceWrite,
            command_mode: CommandMode::Disabled,
            max_tool_calls_per_turn: 16,
            tool_timeout_ms: 30_000,
            output_limit_bytes: 32,
        };
        let registry = ToolRegistry::new(skills, &config);

        let result = registry
            .execute("chatty_error", "call-1", serde_json::json!({}))
            .await;

        assert!(!result.ok);
        assert_eq!(result.error.unwrap().code, "output_limit_exceeded");
        assert!(result.metadata.output_truncated);
        remove_test_dir(root).await;
    }

    async fn write_skill(root: &std::path::Path, folder: &str, tool_name: &str, script: &str) {
        let skill_dir = root.join(folder);
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("skill.json"),
            serde_json::json!({
                "name": folder,
                "description": "Test runtime skill.",
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [
                    {
                        "name": tool_name,
                        "description": "Test tool.",
                        "input_schema": { "type": "object" }
                    }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();
        tokio::fs::write(skill_dir.join("index.js"), script)
            .await
            .unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.unwrap();
        }
    }
}
