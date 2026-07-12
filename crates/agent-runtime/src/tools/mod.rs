pub mod builtin;
pub mod command;
pub mod discovery;
pub mod patch;
pub mod path;
pub mod process;
pub mod result;
pub mod schema;
pub mod search;

use crate::policy::{ApprovalPolicy, SandboxProfile};
use crate::skill::{SkillExecutionContext, SkillRegistry};
use crate::skill_management_tools::{SkillManagementToolContext, SkillManagementTools};
use builtin::BuiltInTools;
use discovery::{ConnectorMetadata, ExternalToolConfig, ExternalToolExecution, ToolDiscoveryItem};
use result::{ToolError, ToolResult, ToolResultMetadata};
use schema::{ToolDiagnostic, validate_tool_definition};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const DEFAULT_MAX_TOOL_CALLS_PER_TURN: usize = 16;
const DEFAULT_TOOL_TIMEOUT_MS: u64 = 30_000;
const DEFAULT_OUTPUT_LIMIT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum CommandMode {
    Disabled,
    Allowed,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct RuntimeConfig {
    pub workspace_root: PathBuf,
    pub cwd: PathBuf,
    pub mode: RuntimeMode,
    pub command_mode: CommandMode,
    #[serde(default = "default_built_in_tools_enabled")]
    pub built_in_tools_enabled: bool,
    pub max_tool_calls_per_turn: usize,
    pub tool_timeout_ms: u64,
    pub output_limit_bytes: usize,
    pub approval_policy: ApprovalPolicy,
    pub sandbox_profile: SandboxProfile,
    pub external_tools: Vec<ExternalToolConfig>,
    pub connectors: Vec<ConnectorMetadata>,
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
            built_in_tools_enabled: true,
            max_tool_calls_per_turn: DEFAULT_MAX_TOOL_CALLS_PER_TURN,
            tool_timeout_ms: DEFAULT_TOOL_TIMEOUT_MS,
            output_limit_bytes: DEFAULT_OUTPUT_LIMIT_BYTES,
            approval_policy: ApprovalPolicy::Never,
            sandbox_profile: SandboxProfile::default(),
            external_tools: Vec::new(),
            connectors: Vec::new(),
        }
    }

    pub fn with_command_mode(mut self, command_mode: CommandMode) -> Self {
        self.command_mode = command_mode;
        self
    }

    pub fn without_builtin_tools(mut self) -> Self {
        self.built_in_tools_enabled = false;
        self
    }
}

fn default_built_in_tools_enabled() -> bool {
    true
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub enum RuntimeMode {
    ReadOnly,
    WorkspaceWrite,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolPermission {
    ReadWorkspace,
    WriteWorkspace,
    ExecuteCommand,
    ManageSkills,
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
    RuntimeSkill {
        skill_name: String,
        package_id: String,
        revision_id: Option<String>,
    },
    Mcp {
        server: String,
    },
    AppConnector {
        connector: String,
    },
}

#[async_trait::async_trait]
pub trait ToolExecutionObserver: Send + Sync {
    async fn finished(&self, source: &ToolSource, success: bool) -> anyhow::Result<()>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolObserverDiagnostic {
    pub operation: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
pub struct ApprovalRequirement {
    pub permission: ToolPermission,
    pub policy: ApprovalPolicy,
}

#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ToolDiscovery {
    pub tools: Vec<ToolDiscoveryItem>,
    pub connectors: Vec<ConnectorMetadata>,
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
        ToolPermission::ManageSkills => false,
    }
}

pub struct ToolRegistry {
    builtins: BuiltInTools,
    built_in_tools_enabled: bool,
    skills: SkillRegistry,
    external_tools: Vec<ExternalToolConfig>,
    external_definitions: Vec<ToolDefinition>,
    external_discovery: Vec<ToolDiscoveryItem>,
    connectors: Vec<ConnectorMetadata>,
    workspace_root: PathBuf,
    cwd: PathBuf,
    mode: RuntimeMode,
    command_mode: CommandMode,
    tool_timeout: Duration,
    output_limit_bytes: usize,
    approval_policy: ApprovalPolicy,
    management: Option<SkillManagementToolContext>,
    execution_observer: Option<Arc<dyn ToolExecutionObserver>>,
    observer_diagnostics: Arc<Mutex<VecDeque<ToolObserverDiagnostic>>>,
}

impl std::fmt::Debug for ToolRegistry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolRegistry")
            .field("built_in_tools_enabled", &self.built_in_tools_enabled)
            .field("external_tools", &self.external_tools.len())
            .field("has_management", &self.management.is_some())
            .field("has_execution_observer", &self.execution_observer.is_some())
            .finish_non_exhaustive()
    }
}

impl ToolRegistry {
    pub fn new(skills: SkillRegistry, config: &RuntimeConfig) -> Self {
        Self::try_new(skills, config).expect("runtime tool registry should be valid")
    }

    pub fn try_new(skills: SkillRegistry, config: &RuntimeConfig) -> anyhow::Result<Self> {
        Self::try_new_with_management(skills, config, None)
    }

    pub fn new_with_management(
        skills: SkillRegistry,
        config: &RuntimeConfig,
        management: Option<SkillManagementToolContext>,
    ) -> Self {
        Self::try_new_with_management(skills, config, management)
            .expect("runtime tool registry should be valid")
    }

    pub fn try_new_with_management(
        skills: SkillRegistry,
        config: &RuntimeConfig,
        management: Option<SkillManagementToolContext>,
    ) -> anyhow::Result<Self> {
        let external_definitions = external_definitions(&config.external_tools)?;
        let external_discovery = external_discovery(&config.external_tools)?;
        Self {
            builtins: BuiltInTools::new(config.clone()),
            built_in_tools_enabled: config.built_in_tools_enabled,
            skills,
            external_tools: config.external_tools.clone(),
            external_definitions,
            external_discovery,
            connectors: config.connectors.clone(),
            workspace_root: config.workspace_root.clone(),
            cwd: config.cwd.clone(),
            mode: config.mode,
            command_mode: config.command_mode,
            tool_timeout: Duration::from_millis(config.tool_timeout_ms),
            output_limit_bytes: config.output_limit_bytes,
            approval_policy: config.approval_policy,
            management,
            execution_observer: None,
            observer_diagnostics: Arc::new(Mutex::new(VecDeque::with_capacity(32))),
        }
        .validate()
    }

    pub fn with_execution_observer(mut self, observer: Arc<dyn ToolExecutionObserver>) -> Self {
        self.execution_observer = Some(observer);
        self
    }

    pub fn observer_diagnostics(&self) -> Vec<ToolObserverDiagnostic> {
        self.observer_diagnostics
            .lock()
            .expect("tool observer diagnostic lock poisoned")
            .iter()
            .cloned()
            .collect()
    }

    pub fn definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = self.non_management_definitions();
        if let Some(context) = &self.management {
            definitions.extend(SkillManagementTools::definitions(
                &context.service,
                &context.actor,
            ));
        }
        definitions
    }

    fn non_management_definitions(&self) -> Vec<ToolDefinition> {
        let mut definitions = if self.built_in_tools_enabled {
            self.builtins.definitions()
        } else {
            Vec::new()
        };
        definitions.extend(
            self.skills
                .tools_with_runtime_sources()
                .into_iter()
                .filter_map(|(skill_name, package_id, revision_id, tool)| {
                    if self.built_in_tools_enabled && BuiltInTools::handles(&tool.name) {
                        return None;
                    }

                    Some(ToolDefinition {
                        name: tool.name,
                        namespace: None,
                        description: tool.description,
                        input_schema: tool.input_schema,
                        output_schema: None,
                        permission: tool.permission,
                        source: ToolSource::RuntimeSkill {
                            skill_name,
                            package_id,
                            revision_id,
                        },
                    })
                }),
        );
        definitions.extend(self.external_definitions.clone());
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

    pub fn parallel_safe(&self, name: &str) -> bool {
        self.definitions().into_iter().any(|definition| {
            definition.name == name
                && definition.permission == ToolPermission::ReadWorkspace
                && matches!(definition.source, ToolSource::BuiltIn)
        })
    }

    pub fn discovery(&self) -> ToolDiscovery {
        let mut tools: Vec<_> = self
            .definitions()
            .into_iter()
            .map(|definition| ToolDiscoveryItem {
                name: definition.name,
                namespace: definition.namespace,
                summary: definition.description.clone(),
                description: definition.description,
                permission: definition.permission,
                source: definition.source,
                schema_loaded: true,
                deferred: false,
            })
            .collect();
        tools.extend(
            self.external_discovery
                .iter()
                .filter(|item| item.deferred)
                .cloned(),
        );
        tools.sort_by(|left, right| {
            left.namespace
                .cmp(&right.namespace)
                .then_with(|| left.name.cmp(&right.name))
        });

        ToolDiscovery {
            tools,
            connectors: self.connectors.clone(),
        }
    }

    pub async fn execute(&self, name: &str, call_id: &str, arguments: Value) -> ToolResult {
        let started = Instant::now();
        if let Some(context) = &self.management
            && SkillManagementTools::is_reserved_name(name)
        {
            if SkillManagementTools::handles(context, name) {
                let result = SkillManagementTools::execute(context, name, call_id, arguments).await;
                return result;
            }
            return registry_failure(
                name,
                call_id,
                "unknown_tool",
                format!("unknown tool: {name}"),
                false,
                registry_metadata(started),
            );
        }
        let execution = tokio::time::timeout(
            self.tool_timeout,
            self.execute_without_timeout(name, call_id, arguments, started),
        );

        match execution.await {
            Ok(result) => self.apply_output_limit(result),
            Err(_) => {
                self.observe_runtime_timeout(name).await;
                registry_failure(
                    name,
                    call_id,
                    "timeout",
                    "tool execution timed out",
                    true,
                    registry_metadata(started),
                )
            }
        }
    }

    async fn execute_without_timeout(
        &self,
        name: &str,
        call_id: &str,
        arguments: Value,
        started: Instant,
    ) -> ToolResult {
        if self.built_in_tools_enabled && BuiltInTools::handles(name) {
            return self.builtins.execute(name, call_id, arguments).await;
        }

        if let Some(tool) = self.external_tool(name) {
            return self.execute_external_tool(tool, name, call_id, started);
        }

        let skill_tools = self.skills.tools();
        let Some(skill_tool) = skill_tools.iter().find(|tool| tool.name.as_str() == name) else {
            return registry_failure(
                name,
                call_id,
                "unknown_tool",
                format!("unknown tool: {name}"),
                false,
                ToolResultMetadata::default(),
            );
        };

        if !permission_allowed(self.mode, self.command_mode, skill_tool.permission) {
            return registry_failure(
                name,
                call_id,
                "permission_denied",
                "tool is not allowed in the current runtime mode",
                false,
                registry_metadata(started),
            );
        }

        let source = self.runtime_tool_source(name);
        let result = match self
            .skills
            .execute_with_context(
                name,
                arguments,
                SkillExecutionContext {
                    workspace_root: self.workspace_root.clone(),
                    cwd: self.cwd.clone(),
                    output_limit_bytes: self.output_limit_bytes,
                },
            )
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
        };
        let attributable = result.ok
            || result.error.as_ref().is_some_and(|error| {
                error.message.contains("skill command failed")
                    || error
                        .message
                        .contains("tool output exceeded runtime output limit")
            });
        if attributable {
            self.observe_execution(&source, result.ok).await;
        }
        result
    }

    fn runtime_tool_source(&self, name: &str) -> ToolSource {
        self.find_runtime_tool_source(name)
            .expect("runtime tool source must exist for an executable skill tool")
    }

    fn find_runtime_tool_source(&self, name: &str) -> Option<ToolSource> {
        self.skills
            .tools_with_runtime_sources()
            .into_iter()
            .find_map(|(skill_name, package_id, revision_id, tool)| {
                (tool.name == name).then_some(ToolSource::RuntimeSkill {
                    skill_name,
                    package_id,
                    revision_id,
                })
            })
    }

    async fn observe_runtime_timeout(&self, name: &str) {
        if (self.built_in_tools_enabled && BuiltInTools::handles(name))
            || self.external_tool(name).is_some()
        {
            return;
        }
        let allowed = self.skills.tools().iter().any(|tool| {
            tool.name == name && permission_allowed(self.mode, self.command_mode, tool.permission)
        });
        if !allowed {
            return;
        }
        if let Some(source) = self.find_runtime_tool_source(name) {
            self.observe_execution(&source, false).await;
        }
    }

    async fn observe_execution(&self, source: &ToolSource, success: bool) {
        let Some(observer) = &self.execution_observer else {
            return;
        };
        if observer.finished(source, success).await.is_ok() {
            return;
        }
        let mut diagnostics = self
            .observer_diagnostics
            .lock()
            .expect("tool observer diagnostic lock poisoned");
        if diagnostics.len() == 32 {
            diagnostics.pop_front();
        }
        diagnostics.push_back(ToolObserverDiagnostic {
            operation: "tool_execution_observer",
            message: "tool execution observer failed".into(),
        });
    }

    fn external_tool(&self, name: &str) -> Option<&ExternalToolConfig> {
        self.external_tools
            .iter()
            .find(|tool| matches!(tool.flattened_name(), Ok(flattened) if flattened == name))
    }

    fn execute_external_tool(
        &self,
        tool: &ExternalToolConfig,
        name: &str,
        call_id: &str,
        started: Instant,
    ) -> ToolResult {
        if matches!(
            tool.visibility,
            discovery::ExternalToolVisibility::Deferred { .. }
        ) {
            return registry_failure(
                name,
                call_id,
                "tool_disabled",
                "Deferred external tool schema is not loaded.",
                false,
                registry_metadata(started),
            );
        }

        match &tool.execution {
            ExternalToolExecution::Static { result } => {
                ToolResult::success(name, call_id, result.clone(), registry_metadata(started))
            }
            ExternalToolExecution::Unavailable => registry_failure(
                name,
                call_id,
                "tool_disabled",
                "External tool execution is not implemented in this phase.",
                false,
                registry_metadata(started),
            ),
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

    fn validate(self) -> anyhow::Result<Self> {
        for definition in self.non_management_definitions() {
            if SkillManagementTools::is_reserved_name(&definition.name) {
                anyhow::bail!(
                    "duplicate tool name: {} (reserved skill management tool name)",
                    definition.name
                );
            }
        }
        let mut names = HashSet::new();
        for definition in self.definitions() {
            if !names.insert(definition.name.clone()) {
                anyhow::bail!("duplicate tool name: {}", definition.name);
            }
        }

        Ok(self)
    }
}

fn external_definitions(tools: &[ExternalToolConfig]) -> anyhow::Result<Vec<ToolDefinition>> {
    tools
        .iter()
        .filter_map(|tool| tool.tool_definition().transpose())
        .collect()
}

fn external_discovery(tools: &[ExternalToolConfig]) -> anyhow::Result<Vec<ToolDiscoveryItem>> {
    tools
        .iter()
        .map(ExternalToolConfig::discovery_summary)
        .collect()
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
mod registry_tests;

#[cfg(test)]
mod execution_observer_tests;

#[cfg(test)]
mod management_registry_tests;

#[cfg(test)]
mod management_permission_tests {
    use super::*;

    #[test]
    fn skill_management_permission_is_never_enabled_by_runtime_modes() {
        for mode in [RuntimeMode::ReadOnly, RuntimeMode::WorkspaceWrite] {
            for command_mode in [CommandMode::Disabled, CommandMode::Allowed] {
                assert!(!permission_allowed(
                    mode,
                    command_mode,
                    ToolPermission::ManageSkills
                ));
            }
        }
    }

    #[test]
    fn skill_management_permission_has_stable_metadata_name() {
        assert_eq!(
            serde_json::to_value(ToolPermission::ManageSkills).unwrap(),
            serde_json::json!("manage_skills")
        );
    }
}
