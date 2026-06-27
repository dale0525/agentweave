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
    let root = std::env::temp_dir().join(format!("general-agent-turn-{name}-{}", Uuid::new_v4()));
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
async fn approval_required_blocks_tool_before_raw_arguments_event() {
    let workspace = test_workspace("approval-required");
    let skills = SkillRegistry::load(skills_root()).await.unwrap();
    let mut config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
    config.approval_policy = crate::policy::ApprovalPolicy::OnWorkspaceWrite;
    let runner = TurnRunner::new_with_config(
        FakePhaseTwoModel {
            calls: AtomicUsize::new(0),
            tool_name: "create_directory",
            arguments: serde_json::json!({ "path": "blocked-secret-path" }),
            requests: Mutex::new(Vec::new()),
        },
        skills,
        config,
    );

    let events = runner.run("create a directory").await.unwrap();

    assert!(!workspace.join("blocked-secret-path").exists());
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ApprovalRequired { name, .. } if name == "create_directory"
    )));
    assert!(!events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallStarted { arguments, .. }
            if arguments.to_string().contains("blocked-secret-path")
    )));
    let result = tool_result(&events);
    assert_eq!(result["ok"], false);
    assert_eq!(result["error"]["code"], "approval_required");
    remove_workspace(&workspace);
}

#[tokio::test]
async fn deferred_external_tools_are_not_sent_as_model_tool_schemas() {
    let workspace = test_workspace("deferred-tools-hidden");
    let skills = SkillRegistry::load(skills_root()).await.unwrap();
    let config = RuntimeConfig {
        external_tools: vec![crate::tools::discovery::ExternalToolConfig::mcp(
            "search",
            "expensive_lookup",
            "Search a remote corpus.",
            serde_json::json!({ "type": "object" }),
            crate::tools::discovery::ExternalToolVisibility::Deferred {
                summary: "Remote corpus lookup.".into(),
            },
        )],
        ..RuntimeConfig::workspace_write(workspace.clone(), workspace.clone())
    };
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

    assert!(!request_has_tool(
        &requests[0],
        "mcp__search__expensive_lookup"
    ));
    remove_workspace(&workspace);
}

#[tokio::test]
async fn phase_three_injects_summary_and_triggered_skill_instruction() {
    let workspace = test_workspace("phase-three-skill-instructions");
    let skills_root = workspace.join("skills");
    fs::create_dir_all(skills_root.join("planning")).unwrap();
    fs::write(
        skills_root.join("planning").join("SKILL.md"),
        "---\nname: planning\ndescription: Write plans.\n---\n\n# Planning\nUse checklists.",
    )
    .unwrap();
    let catalog = crate::skill_catalog::SkillCatalog::load_development(&skills_root)
        .await
        .unwrap();
    let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
    let config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
    let runner = TurnRunner::new_with_catalog_and_config(
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
        catalog,
        config,
    );

    let _events = runner.run("use $planning").await.unwrap();
    let requests = runner.model.requests.lock().unwrap();
    let developer = requests[0].input[1]["content"].as_str().unwrap();

    assert!(developer.contains("<available_skills count=\"1\">"));
    assert!(developer.contains("<skill_instructions name=\"planning\""));
    assert!(developer.contains("Use checklists."));
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
