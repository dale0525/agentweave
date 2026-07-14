use crate::events::RuntimeEvent;
use crate::skill::SkillRegistry;
use crate::skill_authoring_tests::AuthoringFixture;
use crate::skill_recovery_tests::activate_new_revision;
use crate::skill_state::SkillLayerRecord;
use crate::tools::discovery::{
    ExternalToolConfig, ExternalToolExecution, ExternalToolKind, ExternalToolVisibility,
};
use crate::tools::{RuntimeConfig, ToolPermission, ToolRegistry, ToolSource};
use crate::turn::{ModelClient, ModelEventStream, TurnRunner};
use async_trait::async_trait;
use futures::stream;
use model_gateway::provider::{EndpointType, ProviderProfile};
use model_gateway::responses::{
    GatewayEvent, GatewayRequest, parse_gateway_response_with_tool_map,
};
use model_gateway::tool_identity::ToolNameMap;
use std::collections::BTreeMap;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn runtime_tool_identity_dispatches_canonical_ids_to_the_owning_skill() {
    let root = unique_test_dir("canonical-dispatch");
    write_skill(&root, "calendar", "create_event", "calendar").await;
    write_skill(&root, "tasks", "create_event", "tasks").await;
    let registry = runtime_registry(&root, RuntimeConfig::workspace_write(&root, &root)).await;

    let names: Vec<_> = registry
        .definitions()
        .into_iter()
        .map(|definition| definition.name)
        .collect();
    assert!(names.contains(&"calendar/create_event".to_string()));
    assert!(names.contains(&"tasks/create_event".to_string()));
    assert!(!names.contains(&"create_event".to_string()));

    let calendar = registry
        .execute(
            "calendar/create_event",
            "call-calendar",
            serde_json::json!({}),
        )
        .await;
    let tasks = registry
        .execute("tasks/create_event", "call-tasks", serde_json::json!({}))
        .await;

    assert_eq!(calendar.tool, "calendar/create_event");
    assert_eq!(calendar.data.unwrap()["owner"], "calendar");
    assert_eq!(tasks.data.unwrap()["owner"], "tasks");
    remove_test_dir(root).await;
}

#[tokio::test]
async fn runtime_tool_identity_exposes_only_unique_aliases_with_bounded_deprecation() {
    let root = unique_test_dir("unique-alias");
    write_skill(&root, "calendar", "create_event", "calendar").await;
    let registry = runtime_registry(&root, RuntimeConfig::workspace_write(&root, &root)).await;

    let definitions = registry.definitions();
    assert!(definitions.iter().any(|tool| {
        tool.name == "calendar/create_event" && tool.namespace.as_deref() == Some("calendar")
    }));
    assert!(definitions.iter().any(|tool| tool.name == "create_event"));

    for call in 0..40 {
        let result = registry
            .execute(
                "create_event",
                &format!("call-{call}"),
                serde_json::json!({}),
            )
            .await;
        assert!(result.ok);
        assert_eq!(result.tool, "create_event");
        assert_eq!(result.data.unwrap()["tool"], "create_event");
    }

    let diagnostics = registry.observer_diagnostics();
    assert_eq!(diagnostics.len(), 32);
    assert!(diagnostics.iter().all(|diagnostic| {
        diagnostic.operation == "runtime_tool_alias_deprecation"
            && diagnostic.message == "unqualified runtime tool aliases are deprecated"
    }));
    remove_test_dir(root).await;
}

#[tokio::test]
async fn runtime_tool_identity_does_not_advertise_or_execute_ambiguous_aliases() {
    let root = unique_test_dir("ambiguous-alias");
    write_skill(&root, "calendar", "create_event", "calendar").await;
    write_skill(&root, "tasks", "create_event", "tasks").await;
    let registry = runtime_registry(&root, RuntimeConfig::workspace_write(&root, &root)).await;

    assert!(
        !registry
            .definitions()
            .iter()
            .any(|tool| tool.name == "create_event")
    );
    let result = registry
        .execute("create_event", "call-1", serde_json::json!({}))
        .await;
    assert_eq!(result.error.unwrap().code, "unknown_tool");
    remove_test_dir(root).await;
}

#[tokio::test]
async fn runtime_tool_identity_aliases_never_shadow_non_runtime_tools() {
    let root = unique_test_dir("alias-shadow");
    write_skill(&root, "filesystem", "read_text_file", "runtime").await;
    write_skill(&root, "remote", "mcp__search__lookup", "runtime").await;
    write_skill(&root, "mail", "connector__mail__send", "runtime").await;
    write_skill(&root, "manager", "create_skill_draft", "runtime").await;
    let mut config = RuntimeConfig::workspace_write(&root, &root);
    config.external_tools = vec![
        ExternalToolConfig::mcp(
            "search",
            "lookup",
            "Search externally.",
            serde_json::json!({ "type": "object" }),
            ExternalToolVisibility::Immediate,
        ),
        ExternalToolConfig {
            kind: ExternalToolKind::AppConnector {
                connector: "mail".into(),
            },
            name: "send".into(),
            description: "Send mail externally.".into(),
            input_schema: serde_json::json!({ "type": "object" }),
            permission: ToolPermission::ReadWorkspace,
            visibility: ExternalToolVisibility::Immediate,
            execution: ExternalToolExecution::Static {
                result: serde_json::json!({ "source": "connector" }),
            },
        },
    ];
    let registry = runtime_registry(&root, config).await;
    let definitions = registry.definitions();

    for canonical in [
        "filesystem/read_text_file",
        "remote/mcp__search__lookup",
        "mail/connector__mail__send",
        "manager/create_skill_draft",
    ] {
        assert!(definitions.iter().any(|tool| tool.name == canonical));
    }
    assert_eq!(
        definitions
            .iter()
            .filter(|tool| tool.name == "read_text_file")
            .count(),
        1
    );
    assert!(
        definitions
            .iter()
            .any(|tool| { tool.name == "read_text_file" && tool.source == ToolSource::BuiltIn })
    );
    assert!(definitions.iter().any(|tool| {
        tool.name == "mcp__search__lookup" && matches!(tool.source, ToolSource::Mcp { .. })
    }));
    assert!(definitions.iter().any(|tool| {
        tool.name == "connector__mail__send"
            && matches!(tool.source, ToolSource::AppConnector { .. })
    }));
    assert!(
        !definitions
            .iter()
            .any(|tool| tool.name == "create_skill_draft")
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn runtime_tool_identity_rejects_unvalidated_development_package_identity() {
    let root = unique_test_dir("invalid-package-identity");
    write_skill(&root, "bad-package", "run", "bad").await;
    let manifest_path = root.join("bad-package/skill.json");
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&tokio::fs::read(&manifest_path).await.unwrap()).unwrap();
    manifest["name"] = serde_json::json!("bad/package");
    tokio::fs::write(&manifest_path, manifest.to_string())
        .await
        .unwrap();

    let error = SkillRegistry::load_development(&root).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "invalid runtime package identity: bad/package"
    );
    remove_test_dir(root).await;
}

#[tokio::test]
async fn runtime_tool_identity_alias_preserves_managed_revision_circuit_attribution() {
    let fixture = AuthoringFixture::new().await;
    let other_revision = activate_new_revision(&fixture, "0.9.0").await;
    let (skills, package_id, revision) = activate_managed_runtime_skills(&fixture).await;
    let mut config = RuntimeConfig::workspace_write(fixture.imports.path(), fixture.imports.path())
        .without_builtin_tools();
    config.tool_timeout_ms = 25;
    let registry = ToolRegistry::new(skills, &config)
        .with_execution_observer(Arc::new(fixture.manager.clone()));
    let definitions = registry.definitions();
    let canonical = definitions
        .iter()
        .find(|tool| tool.name == "com.example.calendar/create_event")
        .unwrap();
    let alias = definitions
        .iter()
        .find(|tool| tool.name == "create_event")
        .unwrap();

    assert_eq!(alias.source, canonical.source);
    assert_eq!(
        alias.source,
        ToolSource::RuntimeSkill {
            skill_name: "calendar-runtime".into(),
            package_id: package_id.as_str().into(),
            revision_id: Some(revision.clone()),
        }
    );
    let result = registry
        .execute("create_event", "call-alias", serde_json::json!({}))
        .await;

    assert!(!result.ok);
    assert_eq!(result.tool, "create_event");
    assert_eq!(result.error.unwrap().code, "timeout");
    assert!(
        registry
            .observer_diagnostics()
            .iter()
            .any(|diagnostic| { diagnostic.operation == "runtime_tool_alias_deprecation" })
    );
    let circuit = fixture
        .state
        .get_circuit_state(&revision)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(circuit.consecutive_failures, 1);
    assert!(
        fixture
            .state
            .get_circuit_state(&other_revision)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn provider_selected_alias_stays_canonical_through_turn_and_exact_circuit_attribution() {
    let fixture = AuthoringFixture::new().await;
    let other_revision = activate_new_revision(&fixture, "0.9.0").await;
    let (skills, _, revision) = activate_managed_runtime_skills(&fixture).await;
    let model = Arc::new(AliasSelectingTurnModel::default());
    let mut config = RuntimeConfig::workspace_write(fixture.imports.path(), fixture.imports.path())
        .without_builtin_tools();
    config.tool_timeout_ms = 25;
    let runner = TurnRunner::new_with_config(model.clone(), skills, config)
        .with_execution_observer_for_test(Arc::new(fixture.manager.clone()));

    let events = runner.run("create an event").await.unwrap();
    let requests = model.requests.lock().unwrap().clone();

    let advertised: Vec<_> = requests[0]
        .tools
        .iter()
        .filter(|tool| tool.id == "com.example.calendar/create_event")
        .map(|tool| tool.advertised_name())
        .collect();
    assert_eq!(
        advertised,
        vec!["com.example.calendar/create_event", "create_event"]
    );
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallStarted { name, .. }
            if name == "com.example.calendar/create_event"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallFinished { result, .. }
            if result["tool"] == "com.example.calendar/create_event"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolObserverDiagnostic { operation, message }
            if operation == "runtime_tool_alias_deprecation"
                && message == "unqualified runtime tool aliases are deprecated"
    )));
    assert_eq!(
        requests[1]
            .input
            .iter()
            .find(|item| {
                item.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
            })
            .unwrap()["tool_calls"][0]["function"]["name"],
        "com.example.calendar/create_event"
    );
    assert_eq!(
        fixture
            .state
            .get_circuit_state(&revision)
            .await
            .unwrap()
            .unwrap()
            .consecutive_failures,
        1
    );
    assert!(
        fixture
            .state
            .get_circuit_state(&other_revision)
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn runtime_tool_identity_remains_canonical_through_turn_events_results_and_history() {
    let root = unique_test_dir("canonical-turn");
    write_skill(&root, "calendar", "create_event", "calendar").await;
    let skills = SkillRegistry::load_development(&root).await.unwrap();
    let model = Arc::new(CanonicalTurnModel::default());
    let runner = TurnRunner::new_with_config(
        model.clone(),
        skills,
        RuntimeConfig::workspace_write(&root, &root),
    );

    let events = runner.run("create an event").await.unwrap();
    let requests = model.requests.lock().unwrap().clone();

    assert!(
        requests[0]
            .tools
            .iter()
            .any(|tool| tool.id == "calendar/create_event")
    );
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallStarted { name, .. } if name == "calendar/create_event"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RuntimeEvent::ToolCallFinished { result, .. }
            if result["tool"] == "calendar/create_event"
    )));
    assert_eq!(
        requests[1]
            .input
            .iter()
            .find(|item| {
                item.get("role").and_then(serde_json::Value::as_str) == Some("assistant")
            })
            .unwrap()["tool_calls"][0]["function"]["name"],
        "calendar/create_event"
    );
    remove_test_dir(root).await;
}

#[derive(Default)]
struct CanonicalTurnModel {
    calls: AtomicUsize,
    requests: Mutex<Vec<GatewayRequest>>,
}

#[async_trait]
impl ModelClient for Arc<CanonicalTurnModel> {
    async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream> {
        self.requests.lock().unwrap().push(request);
        let events = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            vec![
                GatewayEvent::ToolCall {
                    call_id: "call-1".into(),
                    name: "calendar/create_event".into(),
                    legacy_alias_selected: false,
                    arguments: serde_json::json!({}),
                },
                GatewayEvent::Completed,
            ]
        } else {
            vec![
                GatewayEvent::TextDelta {
                    text: "done".into(),
                },
                GatewayEvent::Completed,
            ]
        };
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

#[derive(Default)]
struct AliasSelectingTurnModel {
    calls: AtomicUsize,
    requests: Mutex<Vec<GatewayRequest>>,
}

#[async_trait]
impl ModelClient for Arc<AliasSelectingTurnModel> {
    async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream> {
        self.requests.lock().unwrap().push(request.clone());
        let events = if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
            let map = ToolNameMap::from_tools(&request.tools)?;
            let alias = request
                .tools
                .iter()
                .find(|tool| tool.advertised_name() == "create_event")
                .ok_or_else(|| anyhow::anyhow!("alias was not advertised"))?;
            let alias_wire = map
                .wire_name_for_tool(alias)
                .ok_or_else(|| anyhow::anyhow!("alias wire name is unavailable"))?;
            parse_gateway_response_with_tool_map(
                &chat_profile(),
                serde_json::json!({
                    "choices": [{ "message": { "tool_calls": [{
                        "id": "call-alias",
                        "function": { "name": alias_wire, "arguments": "{}" }
                    }] } }]
                }),
                &map,
            )?
        } else {
            vec![
                GatewayEvent::TextDelta {
                    text: "done".into(),
                },
                GatewayEvent::Completed,
            ]
        };
        Ok(Box::pin(stream::iter(events.into_iter().map(Ok))))
    }
}

async fn activate_managed_runtime_skills(
    fixture: &AuthoringFixture,
) -> (SkillRegistry, crate::skill_package::SkillPackageId, String) {
    let package_id = crate::skill_package::SkillPackageId::parse("com.example.calendar").unwrap();
    let source = fixture
        .imports
        .path()
        .join(format!("managed-runtime-{}", uuid::Uuid::new_v4()));
    tokio::fs::create_dir_all(&source).await.unwrap();
    tokio::fs::write(
        source.join("agentweave.json"),
        serde_json::json!({
            "schemaVersion": 1,
            "id": package_id.as_str(),
            "version": "1.0.0",
            "displayName": "Calendar runtime",
            "kind": "native_runtime",
            "package": { "includeInstructions": false, "includeRuntime": true }
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        source.join("skill.json"),
        serde_json::json!({
            "name": "calendar-runtime",
            "description": "Managed calendar runtime.",
            "version": "1.0.0",
            "entry": { "type": "command", "command": "sh", "args": ["run.sh"] },
            "tools": [{
                "name": "create_event",
                "description": "Create an event.",
                "permission": "read_workspace",
                "input_schema": { "type": "object" }
            }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(source.join("run.sh"), "sleep 60\n")
        .await
        .unwrap();
    let staged = fixture
        .store
        .create_staging_revision(&source, "owner-1")
        .await
        .unwrap();
    let managed = fixture
        .store
        .promote_revision(&staged.revision_id)
        .await
        .unwrap();
    fixture
        .state
        .activate_revision(
            &package_id,
            &managed.revision_id,
            SkillLayerRecord::Managed,
            "owner-1",
        )
        .await
        .unwrap();
    fixture.manager.reload().await.unwrap();
    let lease = fixture.manager.lease_snapshot();
    (
        lease.snapshot().registry().clone(),
        package_id,
        managed.revision_id,
    )
}

fn chat_profile() -> ProviderProfile {
    ProviderProfile {
        id: "scripted".into(),
        name: "Scripted".into(),
        endpoint_type: EndpointType::ChatCompletions,
        base_url: "https://example.invalid/v1".into(),
        model: "scripted-model".into(),
        api_key: None,
        headers: BTreeMap::new(),
    }
}

async fn runtime_registry(root: &std::path::Path, config: RuntimeConfig) -> ToolRegistry {
    let skills = SkillRegistry::load_development(root).await.unwrap();
    ToolRegistry::new(skills, &config)
}

async fn write_skill(root: &std::path::Path, package: &str, local: &str, owner: &str) {
    let skill_root = root.join(package);
    tokio::fs::create_dir_all(&skill_root).await.unwrap();
    tokio::fs::write(
        skill_root.join("skill.json"),
        serde_json::json!({
            "name": package,
            "description": "Runtime tool identity test skill.",
            "version": "0.1.0",
            "entry": { "type": "command", "command": "node", "args": ["index.js"] },
            "tools": [{
                "name": local,
                "description": "Runtime tool identity test tool.",
                "permission": "read_workspace",
                "input_schema": { "type": "object" }
            }]
        })
        .to_string(),
    )
    .await
    .unwrap();
    tokio::fs::write(
        skill_root.join("index.js"),
        format!(
            "process.stdin.resume(); process.stdin.on('end', () => process.stdout.write(JSON.stringify({{ owner: '{owner}', tool: process.env.AGENTWEAVE_TOOL_NAME }})));\n"
        ),
    )
    .await
    .unwrap();
}

fn unique_test_dir(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("agentweave-{name}-{}", uuid::Uuid::new_v4()))
}

async fn remove_test_dir(path: std::path::PathBuf) {
    if path.exists() {
        tokio::fs::remove_dir_all(path).await.unwrap();
    }
}
