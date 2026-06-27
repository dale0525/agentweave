pub mod builtin;
pub mod command;
pub mod patch;
pub mod path;
pub mod process;
pub mod result;
pub mod schema;
pub mod search;

use crate::policy::{ApprovalPolicy, SandboxProfile};
use crate::skill::SkillRegistry;
use builtin::BuiltInTools;
use result::{ToolError, ToolResult, ToolResultMetadata};
use schema::{ToolDiagnostic, validate_tool_definition};
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
    Allowed,
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
    pub approval_policy: ApprovalPolicy,
    pub sandbox_profile: SandboxProfile,
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
            approval_policy: ApprovalPolicy::Never,
            sandbox_profile: SandboxProfile::default(),
        }
    }

    pub fn with_command_mode(mut self, command_mode: CommandMode) -> Self {
        self.command_mode = command_mode;
        self
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
    ExecuteCommand,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub namespace: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Option<Value>,
    pub permission: ToolPermission,
    pub source: ToolSource,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub enum ToolSource {
    BuiltIn,
    RuntimeSkill { skill_name: String },
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApprovalRequirement {
    pub permission: ToolPermission,
    pub policy: ApprovalPolicy,
}

pub fn permission_allowed(
    mode: RuntimeMode,
    command_mode: CommandMode,
    permission: ToolPermission,
) -> bool {
    match permission {
        ToolPermission::ReadWorkspace => true,
        ToolPermission::WriteWorkspace => mode == RuntimeMode::WorkspaceWrite,
        ToolPermission::ExecuteCommand => {
            mode == RuntimeMode::WorkspaceWrite && command_mode == CommandMode::Allowed
        }
    }
}

#[derive(Debug)]
pub struct ToolRegistry {
    builtins: BuiltInTools,
    skills: SkillRegistry,
    tool_timeout: Duration,
    output_limit_bytes: usize,
    approval_policy: ApprovalPolicy,
}

impl ToolRegistry {
    pub fn new(skills: SkillRegistry, config: &RuntimeConfig) -> Self {
        Self {
            builtins: BuiltInTools::new(config.clone()),
            skills,
            tool_timeout: Duration::from_millis(config.tool_timeout_ms),
            output_limit_bytes: config.output_limit_bytes,
            approval_policy: config.approval_policy,
        }
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self.builtins.definitions();
        definitions.extend(self.skills.tools_with_skill_names().into_iter().map(
            |(skill_name, tool)| ToolDefinition {
                name: tool.name,
                namespace: None,
                description: tool.description,
                input_schema: tool.input_schema,
                output_schema: None,
                permission: ToolPermission::ReadWorkspace,
                source: ToolSource::RuntimeSkill { skill_name },
            },
        ));
        definitions
    }

    pub fn diagnostics(&self) -> Vec<ToolDiagnostic> {
        let mut diagnostics: Vec<_> = self
            .definitions()
            .into_iter()
            .map(|definition| ToolDiagnostic {
                name: definition.name.clone(),
                namespace: definition.namespace.clone(),
                description: definition.description.clone(),
                permission: definition.permission,
                source: definition.source.clone(),
                schema: validate_tool_definition(&definition),
            })
            .collect();

        diagnostics.sort_by(|left, right| {
            left.namespace
                .cmp(&right.namespace)
                .then_with(|| left.name.cmp(&right.name))
        });
        diagnostics
    }

    pub fn approval_requirement(&self, name: &str) -> Option<ApprovalRequirement> {
        let definition = self
            .definitions()
            .into_iter()
            .find(|definition| definition.name == name)?;
        self.approval_policy
            .requires_approval(definition.permission)
            .then_some(ApprovalRequirement {
                permission: definition.permission,
                policy: self.approval_policy,
            })
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
    fn tool_definitions_include_source_and_schema_diagnostics() {
        let workspace_root = PathBuf::from("/workspace");
        let config = RuntimeConfig::workspace_write(workspace_root.clone(), workspace_root);
        let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

        let diagnostics = registry.diagnostics();
        let create_directory = diagnostics
            .iter()
            .find(|tool| tool.name == "create_directory")
            .expect("create_directory diagnostic should exist");

        assert_eq!(create_directory.source, ToolSource::BuiltIn);
        assert_eq!(create_directory.permission, ToolPermission::WriteWorkspace);
        assert!(create_directory.schema.valid);
        assert_eq!(create_directory.namespace, None);
    }

    #[tokio::test]
    async fn runtime_skill_diagnostics_include_skill_source() {
        let root = unique_test_dir("runtime-source-diagnostics");
        write_skill(
            &root,
            "echoer",
            "echoer_echo",
            "process.stdin.resume();\nprocess.stdin.on('data', (chunk) => process.stdout.write(chunk));\n",
        )
        .await;
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig::workspace_write(root.clone(), root.clone());
        let registry = ToolRegistry::new(skills, &config);

        let diagnostic = registry
            .diagnostics()
            .into_iter()
            .find(|tool| tool.name == "echoer_echo")
            .unwrap();

        assert_eq!(
            diagnostic.source,
            ToolSource::RuntimeSkill {
                skill_name: "echoer".into()
            }
        );
        assert!(diagnostic.schema.valid);
        remove_test_dir(root).await;
    }

    #[test]
    fn read_only_blocks_workspace_writes() {
        assert!(permission_allowed(
            RuntimeMode::ReadOnly,
            CommandMode::Disabled,
            ToolPermission::ReadWorkspace
        ));
        assert!(!permission_allowed(
            RuntimeMode::ReadOnly,
            CommandMode::Disabled,
            ToolPermission::WriteWorkspace
        ));
        assert!(permission_allowed(
            RuntimeMode::WorkspaceWrite,
            CommandMode::Disabled,
            ToolPermission::ReadWorkspace
        ));
        assert!(permission_allowed(
            RuntimeMode::WorkspaceWrite,
            CommandMode::Disabled,
            ToolPermission::WriteWorkspace
        ));
    }

    #[test]
    fn command_permission_requires_workspace_write_and_command_allowed() {
        assert!(!permission_allowed(
            RuntimeMode::ReadOnly,
            CommandMode::Disabled,
            ToolPermission::ExecuteCommand
        ));
        assert!(!permission_allowed(
            RuntimeMode::ReadOnly,
            CommandMode::Allowed,
            ToolPermission::ExecuteCommand
        ));
        assert!(!permission_allowed(
            RuntimeMode::WorkspaceWrite,
            CommandMode::Disabled,
            ToolPermission::ExecuteCommand
        ));
        assert!(permission_allowed(
            RuntimeMode::WorkspaceWrite,
            CommandMode::Allowed,
            ToolPermission::ExecuteCommand
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
        assert_eq!(
            workspace_write.approval_policy,
            crate::policy::ApprovalPolicy::Never
        );
        assert_eq!(
            workspace_write.sandbox_profile.network,
            crate::policy::NetworkPolicy::UnrestrictedPlaceholder
        );

        assert_eq!(read_only.mode, RuntimeMode::ReadOnly);
        assert_eq!(read_only.command_mode, CommandMode::Disabled);
        assert_eq!(read_only.max_tool_calls_per_turn, 16);
        assert_eq!(read_only.tool_timeout_ms, 30_000);
        assert_eq!(read_only.output_limit_bytes, 64 * 1024);
    }

    #[test]
    fn runtime_config_can_enable_development_command_mode() {
        let workspace_root = PathBuf::from("/workspace");
        let cwd = workspace_root.join("project");
        let config = RuntimeConfig::workspace_write(workspace_root, cwd)
            .with_command_mode(CommandMode::Allowed);

        assert_eq!(config.command_mode, CommandMode::Allowed);
    }

    #[tokio::test]
    async fn tool_registry_reports_approval_requirement_for_write_tools() {
        let root = unique_test_dir("approval-requirement");
        std::fs::create_dir_all(&root).unwrap();
        let mut config = RuntimeConfig::workspace_write(root.clone(), root.clone());
        config.approval_policy = crate::policy::ApprovalPolicy::OnWorkspaceWrite;
        let registry = ToolRegistry::new(SkillRegistry::empty_for_tests(), &config);

        let requirement = registry.approval_requirement("create_directory").unwrap();

        assert_eq!(requirement.permission, ToolPermission::WriteWorkspace);
        assert_eq!(
            requirement.policy,
            crate::policy::ApprovalPolicy::OnWorkspaceWrite
        );
        remove_test_dir(root).await;
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
            output_limit_bytes: 4,
            ..RuntimeConfig::workspace_write(root.clone(), root.clone())
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
            output_limit_bytes: 4,
            ..RuntimeConfig::workspace_write(root.clone(), root.clone())
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
            output_limit_bytes: 4,
            ..RuntimeConfig::workspace_write(root.clone(), root.clone())
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
    async fn tool_registry_preserves_truncated_exec_command_success() {
        let root = unique_test_dir("registry-command-truncated-success");
        tokio::fs::create_dir_all(&root).await.unwrap();
        let skills = SkillRegistry::load_development(&root).await.unwrap();
        let config = RuntimeConfig {
            command_mode: CommandMode::Allowed,
            output_limit_bytes: 2048,
            ..RuntimeConfig::workspace_write(root.clone(), root.clone())
        };
        let registry = ToolRegistry::new(skills, &config);

        let result = registry
            .execute(
                "exec_command",
                "call-1",
                serde_json::json!({ "cmd": "printf '%*s' 2048 '' | tr ' ' x" }),
            )
            .await;

        assert!(result.ok);
        assert!(result.metadata.stdout_truncated);
        assert!(result.metadata.output_truncated);
        assert!(serde_json::to_vec(&result).unwrap().len() <= config.output_limit_bytes);
        let data = result.data.unwrap();
        assert_eq!(data["terminated_by_runtime"], true);
        assert_eq!(data["exit_code"], Value::Null);
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
            tool_timeout_ms: 5,
            ..RuntimeConfig::workspace_write(root.clone(), root.clone())
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
            output_limit_bytes: 32,
            ..RuntimeConfig::workspace_write(root.clone(), root.clone())
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
            output_limit_bytes: 32,
            ..RuntimeConfig::workspace_write(root.clone(), root.clone())
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
