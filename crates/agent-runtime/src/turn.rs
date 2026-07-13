use crate::context::{compact_model_input_with_stats, exceeds_budget};
use crate::events::RuntimeEvent;
use crate::instructions::{InstructionConfig, InstructionContext};
use crate::skill::SkillRegistry;
use crate::skill_catalog::SkillCatalog;
use crate::skill_management::OwnerSkillManagementService;
use crate::skill_management_tools::SkillManagementToolContext;
use crate::skill_manager::SkillManager;
use crate::skill_snapshot::SkillSnapshotLease;
use crate::tools::result::{ToolError, ToolResult, ToolResultMetadata};
use crate::tools::{RuntimeConfig, ToolDefinition, ToolRegistry};
use crate::turn_request::{BudgetPolicy, TurnRequest};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use model_gateway::responses::{GatewayEvent, GatewayRequest, GatewayTool};
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

pub type ModelEventStream = Pin<Box<dyn Stream<Item = anyhow::Result<GatewayEvent>> + Send>>;

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream>;
}

#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn run(&self, user_text: &str) -> anyhow::Result<Vec<RuntimeEvent>>;

    async fn run_request(&self, request: TurnRequest) -> anyhow::Result<Vec<RuntimeEvent>> {
        if request.actor_context != crate::skill_policy::ActorContext::anonymous() {
            anyhow::bail!("agent runner does not accept a host-authenticated actor");
        }
        self.run(&request.user_text).await
    }
}

pub struct TurnRunner<C> {
    model: C,
    skill_manager: SkillManager,
    config: RuntimeConfig,
    max_steps: usize,
    management: Option<OwnerSkillManagementService>,
    execution_observer: Option<Arc<dyn crate::tools::ToolExecutionObserver>>,
}

impl<C> TurnRunner<C>
where
    C: ModelClient,
{
    #[allow(deprecated)]
    #[deprecated(note = "production turns must use new_with_manager_and_config")]
    pub fn new(model: C, skills: SkillRegistry) -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| ".".into());
        let config = RuntimeConfig::workspace_write(workspace.clone(), workspace);
        Self::new_with_config(model, skills, config)
    }

    #[allow(deprecated)]
    #[deprecated(note = "production turns must use new_with_manager_and_config")]
    pub fn new_with_config(model: C, skills: SkillRegistry, config: RuntimeConfig) -> Self {
        Self::new_with_catalog_and_config(model, skills, SkillCatalog::empty(), config)
    }

    #[deprecated(note = "production turns must use new_with_manager_and_config")]
    pub fn new_with_catalog_and_config(
        model: C,
        skills: SkillRegistry,
        skill_catalog: SkillCatalog,
        config: RuntimeConfig,
    ) -> Self {
        let skill_manager = SkillManager::from_registry_and_catalog(skills, skill_catalog);
        Self::new_with_manager_and_config(model, skill_manager, config)
    }

    pub fn new_with_manager_and_config(
        model: C,
        skill_manager: SkillManager,
        config: RuntimeConfig,
    ) -> Self {
        let max_steps = config.max_tool_calls_per_turn.saturating_add(1);
        Self {
            model,
            skill_manager,
            config,
            max_steps,
            management: None,
            execution_observer: None,
        }
    }

    pub fn with_skill_management(mut self, service: OwnerSkillManagementService) -> Self {
        self.management = Some(service);
        self
    }

    #[cfg(test)]
    pub(crate) fn with_execution_observer_for_test(
        mut self,
        observer: Arc<dyn crate::tools::ToolExecutionObserver>,
    ) -> Self {
        self.execution_observer = Some(observer);
        self
    }

    pub async fn run(&self, user_text: &str) -> anyhow::Result<Vec<RuntimeEvent>> {
        self.run_request(TurnRequest::new(user_text)).await
    }

    pub async fn run_request(&self, request: TurnRequest) -> anyhow::Result<Vec<RuntimeEvent>> {
        let lease = self.skill_manager.lease_snapshot_for_turn().await?;
        self.run_with_snapshot(request, lease).await
    }

    async fn run_with_snapshot(
        &self,
        request: TurnRequest,
        lease: SkillSnapshotLease,
    ) -> anyhow::Result<Vec<RuntimeEvent>> {
        let snapshot = lease.snapshot();
        let management = self
            .management
            .as_ref()
            .map(|service| SkillManagementToolContext {
                service: service.clone(),
                actor: request.actor_context.clone(),
            });
        let observer = self
            .execution_observer
            .clone()
            .unwrap_or_else(|| Arc::new(self.skill_manager.clone()));
        let tools = ToolRegistry::try_new_with_management(
            snapshot.registry().clone(),
            &self.config,
            management,
        )?
        .with_execution_observer(observer);
        let skill_catalog = snapshot.catalog();
        let turn_id = Uuid::new_v4().to_string();
        let mut events = vec![RuntimeEvent::TurnStarted {
            turn_id: turn_id.clone(),
        }];
        let mut budget = BudgetPolicy::new(request.token_budget);
        let mut instruction_config =
            InstructionConfig::new(self.config.workspace_root.clone(), self.config.cwd.clone());
        instruction_config.skill_summaries = skill_catalog.summaries().to_vec();
        let triggered_skill_names = skill_catalog.triggered_skill_names(&request.user_text);
        if !triggered_skill_names.is_empty() {
            match skill_catalog
                .load_instruction_documents(&triggered_skill_names, self.config.output_limit_bytes)
                .await
            {
                Ok(documents) => {
                    instruction_config.skill_instructions = documents;
                }
                Err(error) => {
                    events.push(RuntimeEvent::TurnFailed {
                        turn_id,
                        message: error.to_string(),
                    });
                    return Ok(events);
                }
            }
        }
        let instruction_context = InstructionContext::load(instruction_config)?;
        let mut input = instruction_context.model_input(&request.user_text);
        if let Some(goal) = &request.goal {
            let insert_at = input.len().saturating_sub(1);
            input.insert(
                insert_at,
                serde_json::json!({
                    "role": "developer",
                    "content": format!("<active_goal>\n{}\n</active_goal>", goal.objective)
                }),
            );
        }
        if let Some(context_budget_bytes) = request.context_budget_bytes
            && exceeds_budget(&input, context_budget_bytes)?
        {
            let compacted = compact_model_input_with_stats(input, context_budget_bytes)?;
            events.push(RuntimeEvent::ContextCompacted {
                original_items: compacted.original_items,
                compacted_items: compacted.compacted_items,
                budget_bytes: context_budget_bytes,
            });
            input = compacted.input;
        }
        let gateway_tools = gateway_tools(tools.definitions());
        let mut final_text = String::new();
        let mut tool_calls = 0usize;

        for _step in 0..self.max_steps {
            let mut stream = self
                .model
                .stream(GatewayRequest {
                    input: input.clone(),
                    tools: gateway_tools.clone(),
                })
                .await?;
            let mut saw_tool = false;

            while let Some(event) = stream.next().await {
                match event? {
                    GatewayEvent::TextDelta { text } => {
                        final_text.push_str(&text);
                        events.push(RuntimeEvent::AssistantTextDelta { text });
                    }
                    GatewayEvent::ReasoningDelta { text } => {
                        events.push(RuntimeEvent::ReasoningDelta { text });
                    }
                    GatewayEvent::ToolCall {
                        call_id,
                        name,
                        legacy_alias_selected,
                        arguments,
                    } => {
                        saw_tool = true;
                        tool_calls += 1;
                        if tool_calls > self.config.max_tool_calls_per_turn {
                            events.push(RuntimeEvent::TurnFailed {
                                turn_id: turn_id.clone(),
                                message: "max tool calls exceeded".into(),
                            });
                            return Ok(events);
                        }
                        if let Some(requirement) = tools.approval_requirement(&name) {
                            events.push(RuntimeEvent::ApprovalRequired {
                                call_id: call_id.clone(),
                                name: name.clone(),
                                permission: requirement.permission,
                                policy: requirement.policy,
                            });
                            let result = ToolResult::failure(
                                name.clone(),
                                call_id.clone(),
                                ToolError {
                                    code: "approval_required".into(),
                                    message: "Tool call requires approval before execution.".into(),
                                    retryable: false,
                                },
                                ToolResultMetadata::default(),
                            )
                            .into_value();
                            events.push(RuntimeEvent::ToolCallFinished {
                                call_id: call_id.clone(),
                                result: result.clone(),
                            });
                            input.push(serde_json::json!({
                                "role": "assistant",
                                "tool_calls": [
                                    {
                                        "id": call_id.clone(),
                                        "type": "function",
                                        "function": {
                                            "name": name.clone(),
                                            "arguments": "{}"
                                        }
                                    }
                                ]
                            }));
                            input.push(serde_json::json!({
                                "role": "tool",
                                "tool_call_id": call_id,
                                "content": result
                            }));
                            continue;
                        }
                        events.push(RuntimeEvent::ToolCallStarted {
                            call_id: call_id.clone(),
                            name: name.clone(),
                            arguments: arguments.clone(),
                        });
                        input.push(serde_json::json!({
                            "role": "assistant",
                            "tool_calls": [
                                {
                                    "id": call_id.clone(),
                                    "type": "function",
                                    "function": {
                                        "name": name.clone(),
                                        "arguments": arguments.to_string()
                                    }
                                }
                            ]
                        }));
                        let result = tools
                            .execute_provider_call(
                                &name,
                                legacy_alias_selected,
                                &call_id,
                                arguments,
                            )
                            .await
                            .into_value();
                        events.extend(tools.take_observer_diagnostics().into_iter().map(
                            |diagnostic| RuntimeEvent::ToolObserverDiagnostic {
                                operation: diagnostic.operation.into(),
                                message: diagnostic.message,
                            },
                        ));
                        events.push(RuntimeEvent::ToolCallFinished {
                            call_id: call_id.clone(),
                            result: result.clone(),
                        });
                        input.push(serde_json::json!({
                            "role": "tool",
                            "tool_call_id": call_id,
                            "content": result
                        }));
                    }
                    GatewayEvent::Completed => {}
                    GatewayEvent::Error { message } => {
                        events.push(RuntimeEvent::TurnFailed {
                            turn_id: turn_id.clone(),
                            message,
                        });
                        return Ok(events);
                    }
                    GatewayEvent::Usage {
                        input_tokens,
                        output_tokens,
                    } => {
                        let usage = budget.record_usage(input_tokens, output_tokens);
                        events.push(RuntimeEvent::UsageReported {
                            input_tokens: usage.input_tokens,
                            output_tokens: usage.output_tokens,
                            total_tokens: usage.total_tokens,
                            exceeded: usage.exceeded,
                        });
                        if usage.exceeded {
                            events.push(RuntimeEvent::TurnFailed {
                                turn_id: turn_id.clone(),
                                message: "token budget exceeded".into(),
                            });
                            return Ok(events);
                        }
                    }
                    GatewayEvent::ResponseStarted { .. } => {}
                }
            }

            if !saw_tool {
                events.push(RuntimeEvent::AssistantMessageFinished { text: final_text });
                events.push(RuntimeEvent::TurnFinished { turn_id });
                return Ok(events);
            }
        }

        events.push(RuntimeEvent::TurnFailed {
            turn_id,
            message: "max agent steps exceeded".into(),
        });
        Ok(events)
    }
}

#[async_trait]
impl<C> AgentRunner for TurnRunner<C>
where
    C: ModelClient,
{
    async fn run(&self, user_text: &str) -> anyhow::Result<Vec<RuntimeEvent>> {
        TurnRunner::run(self, user_text).await
    }

    async fn run_request(&self, request: TurnRequest) -> anyhow::Result<Vec<RuntimeEvent>> {
        TurnRunner::run_request(self, request).await
    }
}

#[async_trait]
impl ModelClient for model_gateway::responses::GatewayHttpClient {
    async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream> {
        model_gateway::responses::GatewayHttpClient::stream(self, request).await
    }
}

fn gateway_tools(tools: Vec<ToolDefinition>) -> Vec<GatewayTool> {
    tools
        .into_iter()
        .map(|tool| {
            let canonical_id = match &tool.source {
                crate::tools::ToolSource::RuntimeSkill { package_id, .. } => {
                    let local_name = tool.name.rsplit('/').next().unwrap_or(&tool.name);
                    format!("{package_id}/{local_name}")
                }
                _ => tool.name.clone(),
            };
            if tool.name == canonical_id {
                GatewayTool::new(canonical_id, tool.description, tool.input_schema)
            } else {
                GatewayTool::advertised_alias(
                    canonical_id,
                    tool.name,
                    tool.description,
                    tool.input_schema,
                )
            }
        })
        .collect()
}

#[cfg(test)]
#[path = "turn_tests.rs"]
mod turn_tests;
