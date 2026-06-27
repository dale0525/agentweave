use crate::events::RuntimeEvent;
use crate::instructions::{InstructionConfig, InstructionContext};
use crate::skill::SkillRegistry;
use crate::tools::{RuntimeConfig, ToolDefinition, ToolRegistry};
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use model_gateway::responses::{GatewayEvent, GatewayRequest, GatewayTool};
use std::pin::Pin;
use uuid::Uuid;

pub type ModelEventStream = Pin<Box<dyn Stream<Item = anyhow::Result<GatewayEvent>> + Send>>;

#[async_trait]
pub trait ModelClient: Send + Sync {
    async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream>;
}

#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn run(&self, user_text: &str) -> anyhow::Result<Vec<RuntimeEvent>>;
}

pub struct TurnRunner<C> {
    model: C,
    tools: ToolRegistry,
    config: RuntimeConfig,
    max_steps: usize,
}

impl<C> TurnRunner<C>
where
    C: ModelClient,
{
    pub fn new(model: C, skills: SkillRegistry) -> Self {
        let workspace = std::env::current_dir().unwrap_or_else(|_| ".".into());
        let config = RuntimeConfig::workspace_write(workspace.clone(), workspace);
        Self::new_with_config(model, skills, config)
    }

    pub fn new_with_config(model: C, skills: SkillRegistry, config: RuntimeConfig) -> Self {
        let max_steps = config.max_tool_calls_per_turn.saturating_add(1);
        let tools = ToolRegistry::new(skills, &config);
        Self {
            model,
            tools,
            config,
            max_steps,
        }
    }

    pub async fn run(&self, user_text: &str) -> anyhow::Result<Vec<RuntimeEvent>> {
        let turn_id = Uuid::new_v4().to_string();
        let mut events = vec![RuntimeEvent::TurnStarted {
            turn_id: turn_id.clone(),
        }];
        let instruction_context = InstructionContext::load(InstructionConfig::new(
            self.config.workspace_root.clone(),
            self.config.cwd.clone(),
        ))?;
        let mut input = instruction_context.model_input(user_text);
        let tools = gateway_tools(self.tools.definitions());
        let mut final_text = String::new();
        let mut tool_calls = 0usize;

        for _step in 0..self.max_steps {
            let mut stream = self
                .model
                .stream(GatewayRequest {
                    input: input.clone(),
                    tools: tools.clone(),
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
                        let result = self
                            .tools
                            .execute(&name, &call_id, arguments)
                            .await
                            .into_value();
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
                    GatewayEvent::ResponseStarted { .. } | GatewayEvent::Usage { .. } => {}
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
        .map(|tool| GatewayTool {
            name: tool.name,
            description: tool.description,
            input_schema: tool.input_schema,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::RuntimeConfig;
    use futures::stream;
    use std::fs;
    use std::path::Path;
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    struct FakeModel {
        calls: AtomicUsize,
        requests: Mutex<Vec<model_gateway::responses::GatewayRequest>>,
    }

    #[async_trait]
    impl ModelClient for FakeModel {
        async fn stream(
            &self,
            request: model_gateway::responses::GatewayRequest,
        ) -> anyhow::Result<ModelEventStream> {
            self.requests.lock().unwrap().push(request);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let events = if call == 0 {
                vec![
                    Ok(GatewayEvent::ToolCall {
                        call_id: "call-1".into(),
                        name: "echo".into(),
                        arguments: serde_json::json!({ "text": "hello" }),
                    }),
                    Ok(GatewayEvent::Completed),
                ]
            } else {
                vec![
                    Ok(GatewayEvent::TextDelta {
                        text: "done".into(),
                    }),
                    Ok(GatewayEvent::Completed),
                ]
            };
            Ok(Box::pin(stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn executes_tool_and_continues_until_text_response() {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");
        let skills = SkillRegistry::load(skills_root).await.unwrap();
        let runner = TurnRunner::new(
            FakeModel {
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
            },
            skills,
        );

        let events = runner.run("echo hello").await.unwrap();

        assert!(matches!(
            events.last(),
            Some(RuntimeEvent::TurnFinished { .. })
        ));
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::ToolCallFinished { call_id, .. } if call_id == "call-1"
        )));
    }

    #[tokio::test]
    async fn sends_runtime_tool_schemas_to_the_model() {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");
        let model = FakeModel {
            calls: AtomicUsize::new(0),
            requests: Mutex::new(Vec::new()),
        };
        let skills = SkillRegistry::load(skills_root).await.unwrap();
        let runner = TurnRunner::new(model, skills);

        let _events = runner.run("echo hello").await.unwrap();
        let requests = runner.model.requests.lock().unwrap();

        assert!(requests[0].tools.iter().any(|tool| tool.name == "echo"));
        assert_eq!(requests[0].tools[0].input_schema["type"], "object");
    }

    struct ScriptedModel {
        calls: AtomicUsize,
        requests: Mutex<Vec<model_gateway::responses::GatewayRequest>>,
        responses: Vec<Vec<GatewayEvent>>,
    }

    #[async_trait]
    impl ModelClient for ScriptedModel {
        async fn stream(
            &self,
            request: model_gateway::responses::GatewayRequest,
        ) -> anyhow::Result<ModelEventStream> {
            self.requests.lock().unwrap().push(request);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let events = self
                .responses
                .get(call)
                .or_else(|| self.responses.last())
                .cloned()
                .unwrap_or_else(|| vec![GatewayEvent::Completed])
                .into_iter()
                .map(Ok);
            Ok(Box::pin(stream::iter(events)))
        }
    }

    struct FakePhaseTwoModel {
        calls: AtomicUsize,
        tool_name: &'static str,
        arguments: serde_json::Value,
        requests: Mutex<Vec<model_gateway::responses::GatewayRequest>>,
    }

    #[async_trait]
    impl ModelClient for FakePhaseTwoModel {
        async fn stream(
            &self,
            request: model_gateway::responses::GatewayRequest,
        ) -> anyhow::Result<ModelEventStream> {
            self.requests.lock().unwrap().push(request);
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            let events = if call == 0 {
                vec![
                    Ok(GatewayEvent::ToolCall {
                        call_id: "call-1".into(),
                        name: self.tool_name.into(),
                        arguments: self.arguments.clone(),
                    }),
                    Ok(GatewayEvent::Completed),
                ]
            } else {
                vec![
                    Ok(GatewayEvent::TextDelta {
                        text: "done".into(),
                    }),
                    Ok(GatewayEvent::Completed),
                ]
            };
            Ok(Box::pin(stream::iter(events)))
        }
    }

    fn skills_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills")
    }

    fn test_workspace(name: &str) -> std::path::PathBuf {
        let root =
            std::env::temp_dir().join(format!("general-agent-turn-{name}-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn remove_workspace(root: &Path) {
        let _ = fs::remove_dir_all(root);
    }

    fn tool_result(events: &[RuntimeEvent]) -> serde_json::Value {
        events
            .iter()
            .find_map(|event| match event {
                RuntimeEvent::ToolCallFinished { result, .. } => Some(result.clone()),
                _ => None,
            })
            .expect("tool result event should be present")
    }

    fn request_has_tool(request: &model_gateway::responses::GatewayRequest, name: &str) -> bool {
        request.tools.iter().any(|tool| tool.name == name)
    }

    #[tokio::test]
    async fn builtin_create_directory_creates_workspace_directory_through_turn_loop() {
        let workspace = test_workspace("create-directory");
        let skills = SkillRegistry::load(skills_root()).await.unwrap();
        let config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
        let runner = TurnRunner::new_with_config(
            ScriptedModel {
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                responses: vec![
                    vec![
                        GatewayEvent::ToolCall {
                            call_id: "call-create".into(),
                            name: "create_directory".into(),
                            arguments: serde_json::json!({ "path": "made-by-tool" }),
                        },
                        GatewayEvent::Completed,
                    ],
                    vec![
                        GatewayEvent::TextDelta {
                            text: "created".into(),
                        },
                        GatewayEvent::Completed,
                    ],
                ],
            },
            skills,
            config,
        );

        let events = runner.run("create a directory").await.unwrap();

        assert!(workspace.join("made-by-tool").is_dir());
        let result = events
            .iter()
            .find_map(|event| match event {
                RuntimeEvent::ToolCallFinished { call_id, result } if call_id == "call-create" => {
                    Some(result)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(result["ok"], true);
        assert_eq!(result["tool"], "create_directory");
        assert_eq!(result["data"]["path"], "made-by-tool");
        remove_workspace(&workspace);
    }

    #[tokio::test]
    async fn phase_two_search_files_executes_through_turn_loop() {
        let workspace = test_workspace("phase-two-search-files");
        fs::write(workspace.join("notes.txt"), "find me\n").unwrap();
        let skills = SkillRegistry::load(skills_root()).await.unwrap();
        let config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
        let runner = TurnRunner::new_with_config(
            FakePhaseTwoModel {
                calls: AtomicUsize::new(0),
                tool_name: "search_files",
                arguments: serde_json::json!({ "pattern": "find" }),
                requests: Mutex::new(Vec::new()),
            },
            skills,
            config,
        );

        let events = runner.run("search for find").await.unwrap();

        let result = tool_result(&events);
        assert_eq!(result["ok"], true);
        assert_eq!(result["tool"], "search_files");
        assert_eq!(result["data"]["matches"][0]["path"], "notes.txt");
        remove_workspace(&workspace);
    }

    #[tokio::test]
    async fn phase_two_exec_command_is_advertised_only_when_allowed() {
        let disabled_workspace = test_workspace("phase-two-command-disabled");
        let disabled_skills = SkillRegistry::load(skills_root()).await.unwrap();
        let disabled_config =
            RuntimeConfig::workspace_write(disabled_workspace.clone(), disabled_workspace.clone());
        let disabled_runner = TurnRunner::new_with_config(
            ScriptedModel {
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                responses: vec![vec![
                    GatewayEvent::TextDelta {
                        text: "done".into(),
                    },
                    GatewayEvent::Completed,
                ]],
            },
            disabled_skills,
            disabled_config,
        );

        let _events = disabled_runner
            .run("what tools are available?")
            .await
            .unwrap();
        {
            let disabled_requests = disabled_runner.model.requests.lock().unwrap();
            assert!(!request_has_tool(&disabled_requests[0], "exec_command"));
        }
        remove_workspace(&disabled_workspace);

        let workspace = test_workspace("phase-two-command-allowed");
        let skills = SkillRegistry::load(skills_root()).await.unwrap();
        let config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone())
            .with_command_mode(crate::tools::CommandMode::Allowed);
        let runner = TurnRunner::new_with_config(
            FakePhaseTwoModel {
                calls: AtomicUsize::new(0),
                tool_name: "exec_command",
                arguments: serde_json::json!({ "cmd": "printf hello" }),
                requests: Mutex::new(Vec::new()),
            },
            skills,
            config,
        );

        let events = runner.run("run printf").await.unwrap();

        let requests = runner.model.requests.lock().unwrap();
        assert!(request_has_tool(&requests[0], "exec_command"));
        drop(requests);
        let result = tool_result(&events);
        assert_eq!(result["ok"], true);
        assert_eq!(result["tool"], "exec_command");
        assert_eq!(result["data"]["stdout"], "hello");
        remove_workspace(&workspace);
    }

    #[tokio::test]
    async fn phase_two_apply_patch_executes_through_turn_loop() {
        let workspace = test_workspace("phase-two-apply-patch");
        let skills = SkillRegistry::load(skills_root()).await.unwrap();
        let config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
        let runner = TurnRunner::new_with_config(
            FakePhaseTwoModel {
                calls: AtomicUsize::new(0),
                tool_name: "apply_patch",
                arguments: serde_json::json!({
                    "patch": "*** Begin Patch\n*** Add File: notes.txt\n+patched\n*** End Patch\n"
                }),
                requests: Mutex::new(Vec::new()),
            },
            skills,
            config,
        );

        let events = runner.run("apply a patch").await.unwrap();

        assert_eq!(
            fs::read_to_string(workspace.join("notes.txt")).unwrap(),
            "patched\n"
        );
        let result = tool_result(&events);
        assert_eq!(result["ok"], true);
        assert_eq!(result["tool"], "apply_patch");
        assert_eq!(result["data"]["changed_files"][0]["path"], "notes.txt");
        remove_workspace(&workspace);
    }

    #[tokio::test]
    async fn first_request_includes_instruction_context_and_tool_schemas() {
        let workspace = test_workspace("instructions");
        fs::write(
            workspace.join("AGENTS.md"),
            "Project instruction from AGENTS.md",
        )
        .unwrap();
        let skills = SkillRegistry::load(skills_root()).await.unwrap();
        let config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
        let runner = TurnRunner::new_with_config(
            ScriptedModel {
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                responses: vec![vec![
                    GatewayEvent::TextDelta {
                        text: "done".into(),
                    },
                    GatewayEvent::Completed,
                ]],
            },
            skills,
            config,
        );

        let _events = runner.run("hello").await.unwrap();
        let requests = runner.model.requests.lock().unwrap();
        let first = &requests[0];

        assert_eq!(first.input[0]["role"], "system");
        assert!(
            first.input[0]["content"]
                .as_str()
                .unwrap()
                .contains("GeneralAgent is a Codex-like runtime")
        );
        assert_eq!(first.input[1]["role"], "developer");
        let developer = first.input[1]["content"].as_str().unwrap();
        assert!(developer.contains("Use tools for concrete workspace actions"));
        assert!(developer.contains("Project instruction from AGENTS.md"));
        assert_eq!(first.input[2]["role"], "user");
        assert!(
            first
                .tools
                .iter()
                .any(|tool| tool.name == "create_directory")
        );
        assert!(first.tools.iter().any(|tool| tool.name == "echo"));
        remove_workspace(&workspace);
    }

    #[tokio::test]
    async fn read_only_mode_denies_create_directory_and_does_not_create_folder() {
        let workspace = test_workspace("read-only");
        let skills = SkillRegistry::load(skills_root()).await.unwrap();
        let config = RuntimeConfig::read_only(workspace.clone(), workspace.clone());
        let runner = TurnRunner::new_with_config(
            ScriptedModel {
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                responses: vec![
                    vec![
                        GatewayEvent::ToolCall {
                            call_id: "call-denied".into(),
                            name: "create_directory".into(),
                            arguments: serde_json::json!({ "path": "blocked" }),
                        },
                        GatewayEvent::Completed,
                    ],
                    vec![
                        GatewayEvent::TextDelta {
                            text: "denied".into(),
                        },
                        GatewayEvent::Completed,
                    ],
                ],
            },
            skills,
            config,
        );

        let events = runner.run("try to create a directory").await.unwrap();

        assert!(!workspace.join("blocked").exists());
        let result = events
            .iter()
            .find_map(|event| match event {
                RuntimeEvent::ToolCallFinished { call_id, result } if call_id == "call-denied" => {
                    Some(result)
                }
                _ => None,
            })
            .unwrap();
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "permission_denied");
        remove_workspace(&workspace);
    }

    #[tokio::test]
    async fn runaway_tool_loop_stops_at_max_tool_calls_per_turn() {
        let workspace = test_workspace("max-tool-calls");
        let skills = SkillRegistry::load(skills_root()).await.unwrap();
        let mut config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
        config.max_tool_calls_per_turn = 2;
        let runner = TurnRunner::new_with_config(
            ScriptedModel {
                calls: AtomicUsize::new(0),
                requests: Mutex::new(Vec::new()),
                responses: vec![vec![
                    GatewayEvent::ToolCall {
                        call_id: "call-loop".into(),
                        name: "echo".into(),
                        arguments: serde_json::json!({ "text": "again" }),
                    },
                    GatewayEvent::Completed,
                ]],
            },
            skills,
            config,
        );

        let events = runner.run("loop forever").await.unwrap();

        let finished_tool_calls = events
            .iter()
            .filter(|event| matches!(event, RuntimeEvent::ToolCallFinished { .. }))
            .count();
        assert_eq!(finished_tool_calls, 2);
        assert!(events.iter().any(|event| matches!(
            event,
            RuntimeEvent::TurnFailed { message, .. } if message.contains("max tool calls exceeded")
        )));
        remove_workspace(&workspace);
    }
}
