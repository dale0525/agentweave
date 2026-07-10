use agent_runtime::{
    instructions::{InstructionConfig, InstructionContext},
    tools::{
        ToolRegistry,
        discovery::{ConnectorMetadata, ToolDiscoveryItem},
        schema::ToolDiagnostic,
    },
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    routing::{delete, get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::AppState;
use crate::dev_skills::DevSkillInventory;

#[derive(Debug, Serialize)]
struct DevToolsResponse {
    tools: Vec<ToolDiagnostic>,
}

#[derive(Debug, Serialize)]
struct DevToolDiscoveryResponse {
    tools: Vec<ToolDiscoveryItem>,
    connectors: Vec<ConnectorMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstructionsPreviewRequest {
    content: String,
}

#[derive(Debug, Serialize)]
struct InstructionsPreviewResponse {
    system: String,
    developer: String,
    user: String,
    triggered_skills: Vec<String>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/dev/tools", get(list_tools))
        .route("/dev/tool-discovery", get(discover_tools))
        .route("/dev/instructions/preview", post(preview_instructions))
        .route("/dev/skills", get(list_skills))
        .route("/dev/skills/validate", post(validate_skills))
        .route("/dev/skills/reload", post(reload_skills))
        .route("/dev/skills/{skill_id}", delete(delete_skill))
        .with_state(state)
}

async fn list_tools(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevToolsResponse>, StatusCode> {
    let skills = state.skills();
    let registry = ToolRegistry::new(skills, &state.runtime_config());

    Ok(Json(DevToolsResponse {
        tools: registry.diagnostics(),
    }))
}

async fn discover_tools(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevToolDiscoveryResponse>, StatusCode> {
    let skills = state.skills();
    let registry = ToolRegistry::new(skills, &state.runtime_config());
    let discovery = registry.discovery();

    Ok(Json(DevToolDiscoveryResponse {
        tools: discovery.tools,
        connectors: discovery.connectors,
    }))
}

async fn preview_instructions(
    State(state): State<Arc<AppState>>,
    Json(request): Json<InstructionsPreviewRequest>,
) -> Result<Json<InstructionsPreviewResponse>, StatusCode> {
    let runtime_config = state.runtime_config();
    let skill_catalog = state.skill_catalog();
    let triggered_skills = skill_catalog.triggered_skill_names(&request.content);
    let mut instruction_config =
        InstructionConfig::new(runtime_config.workspace_root, runtime_config.cwd);
    instruction_config.skill_summaries = skill_catalog.summaries().to_vec();
    if !triggered_skills.is_empty() {
        instruction_config.skill_instructions = skill_catalog
            .load_instruction_documents(&triggered_skills, runtime_config.output_limit_bytes)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    let context =
        InstructionContext::load(instruction_config).map_err(|_| StatusCode::BAD_REQUEST)?;
    let input = context.model_input(&request.content);

    Ok(Json(InstructionsPreviewResponse {
        system: input_content(&input, 0),
        developer: input_content(&input, 1),
        user: input_content(&input, 2),
        triggered_skills,
    }))
}

fn input_content(input: &[serde_json::Value], index: usize) -> String {
    input
        .get(index)
        .and_then(|item| item.get("content"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

async fn list_skills(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevSkillInventory>, StatusCode> {
    let root = state.skills_root().ok_or(StatusCode::NOT_FOUND)?;
    crate::dev_skills::scan_skill_packages(root)
        .await
        .map(Json)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}

async fn validate_skills(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevSkillInventory>, StatusCode> {
    list_skills(State(state)).await
}

async fn reload_skills(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevSkillInventory>, StatusCode> {
    list_skills(State(state)).await
}

async fn delete_skill(
    Path(skill_id): Path<String>,
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevSkillInventory>, StatusCode> {
    let root = state.skills_root().ok_or(StatusCode::NOT_FOUND)?;
    crate::dev_skills::delete_skill_package(root, &skill_id)
        .await
        .map(Json)
        .map_err(dev_skill_delete_status)
}

fn dev_skill_delete_status(error: anyhow::Error) -> StatusCode {
    let message = error.to_string();
    if message.contains("unsafe")
        || message.contains("invalid")
        || message.contains("single path segment")
        || message.contains("must not be empty")
    {
        StatusCode::BAD_REQUEST
    } else if message.contains("No such file") || message.contains("not found") {
        StatusCode::NOT_FOUND
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}

#[cfg(test)]
mod tests {
    use agent_runtime::{
        events::RuntimeEvent,
        platform::{CapabilitySet, PlatformId},
        skill::SkillRegistry,
        skill_catalog::SkillCatalog,
        skill_manager::{SkillManager, SkillManagerConfig},
        skill_source::{DirectorySkillSource, SkillLayer},
        storage::Storage,
        tools::RuntimeConfig,
        turn::AgentRunner,
    };
    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode, header},
    };
    use serde_json::{Value, json};
    use std::{path::PathBuf, sync::Arc};
    use tower::ServiceExt;

    struct TestAgent;

    #[async_trait]
    impl AgentRunner for TestAgent {
        async fn run(&self, _user_text: &str) -> anyhow::Result<Vec<RuntimeEvent>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn dev_tools_route_is_not_mounted_by_default() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = crate::api::router(Arc::new(crate::api::AppState::new(storage)));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/dev/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn dev_skills_route_is_not_mounted_by_default() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = crate::api::router(Arc::new(crate::api::AppState::new(storage)));

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/dev/skills")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn dev_tools_route_returns_tool_diagnostics_when_enabled() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let skills_root = development_skills().await;
        let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
        let state = Arc::new(crate::api::AppState::new_with_agent_and_skills(
            storage,
            Arc::new(TestAgent),
            skills,
        ));
        let app = crate::api::router_with_dev_routes(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/dev/tools")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        let tools = body["tools"].as_array().unwrap();
        assert!(tools.iter().any(|tool| tool["name"] == "echo"));
        assert!(tools.iter().all(|tool| tool["name"] != "create_directory"));
        assert!(tools.iter().all(|tool| tool.get("schema").is_some()));
        remove_test_dir(skills_root).await;
    }

    #[tokio::test]
    async fn dev_skills_route_returns_inventory_when_enabled() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let skills_root = development_skills().await;
        let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
        let state = Arc::new(
            crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(TestAgent), skills)
                .with_skills_root(skills_root.clone()),
        );
        let app = crate::api::router_with_dev_routes(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/dev/skills")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["packages"][0]["id"], "echo");
        assert_eq!(body["packages"][0]["packageKind"], "runtime");
        remove_test_dir(skills_root).await;
    }

    #[tokio::test]
    async fn dev_delete_skill_rejects_unsafe_id() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let skills_root = development_skills().await;
        let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
        let state = Arc::new(
            crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(TestAgent), skills)
                .with_skills_root(skills_root.clone()),
        );
        let app = crate::api::router_with_dev_routes(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/dev/skills/..%2Fecho")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        assert!(skills_root.join("echo").exists());
        remove_test_dir(skills_root).await;
    }

    #[tokio::test]
    async fn dev_delete_skill_removes_package_and_returns_inventory() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let skills_root = development_skills().await;
        let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
        let state = Arc::new(
            crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(TestAgent), skills)
                .with_skills_root(skills_root.clone()),
        );
        let app = crate::api::router_with_dev_routes(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("DELETE")
                    .uri("/dev/skills/echo")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(!skills_root.join("echo").exists());
        let body = read_json(response).await;
        assert_eq!(body["packages"].as_array().unwrap().len(), 0);
        remove_test_dir(skills_root).await;
    }

    #[tokio::test]
    async fn dev_delete_skill_supports_desktop_cors_preflight() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let skills_root = development_skills().await;
        let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
        let state = Arc::new(
            crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(TestAgent), skills)
                .with_skills_root(skills_root.clone()),
        );
        let app = crate::api::router_with_dev_routes(state);

        let response = app
            .oneshot(
                Request::builder()
                    .method("OPTIONS")
                    .uri("/dev/skills/echo")
                    .header(header::ORIGIN, "http://127.0.0.1:5173")
                    .header(header::ACCESS_CONTROL_REQUEST_METHOD, "DELETE")
                    .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
            "http://127.0.0.1:5173"
        );
        assert!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_METHODS]
                .to_str()
                .unwrap()
                .contains("DELETE")
        );
        assert!(
            response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
                .to_str()
                .unwrap()
                .contains("content-type")
        );
        remove_test_dir(skills_root).await;
    }

    #[tokio::test]
    async fn dev_tool_discovery_returns_deferred_tools_and_connectors() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let workspace = unique_test_dir("tool-discovery");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let skills_root = workspace.join("skills");
        tokio::fs::create_dir_all(&skills_root).await.unwrap();
        let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
        let mut config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
        config
            .external_tools
            .push(agent_runtime::tools::discovery::ExternalToolConfig::mcp(
                "search",
                "expensive_lookup",
                "Search a remote corpus.",
                serde_json::json!({ "type": "object" }),
                agent_runtime::tools::discovery::ExternalToolVisibility::Deferred {
                    summary: "Remote corpus lookup.".into(),
                },
            ));
        config
            .connectors
            .push(agent_runtime::tools::discovery::ConnectorMetadata {
                id: "search".into(),
                name: "Search MCP".into(),
                description: "Remote search connector.".into(),
                version: "0.1.0".into(),
                permissions: vec![agent_runtime::tools::ToolPermission::ReadWorkspace],
                auth_state: agent_runtime::tools::discovery::ConnectorAuthState::NotRequired,
                tool_count: 1,
            });
        let state = Arc::new(
            crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(TestAgent), skills)
                .with_runtime_config(config),
        );
        let app = crate::api::router_with_dev_routes(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/dev/tool-discovery")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert!(body["tools"].as_array().unwrap().iter().any(|tool| {
            tool["name"] == "mcp__search__expensive_lookup"
                && tool["deferred"] == true
                && tool["schema_loaded"] == false
        }));
        assert_eq!(body["connectors"][0]["id"], "search");
        remove_test_dir(workspace).await;
    }

    #[tokio::test]
    async fn dev_instructions_preview_returns_project_and_skill_context() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let workspace = unique_test_dir("instructions-preview");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::write(workspace.join("AGENTS.md"), "Project rules")
            .await
            .unwrap();
        let skills_root = workspace.join("skills");
        tokio::fs::create_dir_all(skills_root.join("planning"))
            .await
            .unwrap();
        tokio::fs::write(
            skills_root.join("planning").join("SKILL.md"),
            "---\nname: planning\ndescription: Write plans.\n---\n\n# Planning\nUse checklists.",
        )
        .await
        .unwrap();
        let skills = SkillRegistry::load_development(&skills_root).await.unwrap();
        let catalog = SkillCatalog::load_development(&skills_root).await.unwrap();
        let state = Arc::new(
            crate::api::AppState::new_with_agent_and_skills(storage, Arc::new(TestAgent), skills)
                .with_runtime_config(RuntimeConfig::workspace_write(
                    workspace.clone(),
                    workspace.clone(),
                ))
                .with_skill_catalog(catalog),
        );
        let app = crate::api::router_with_dev_routes(state);

        let response = app
            .oneshot(json_request(
                "/dev/instructions/preview",
                json!({ "content": "use $planning" }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert!(
            body["developer"]
                .as_str()
                .unwrap()
                .contains("Project rules")
        );
        assert!(
            body["developer"]
                .as_str()
                .unwrap()
                .contains("<available_skills")
        );
        assert!(
            body["developer"]
                .as_str()
                .unwrap()
                .contains("<skill_instructions name=\"planning\"")
        );
        remove_test_dir(workspace).await;
    }

    #[tokio::test]
    async fn dev_runtime_views_follow_the_current_skill_snapshot() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let workspace = unique_test_dir("current-snapshot");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let skills_root = workspace.join("skills");
        let package_root = skills_root.join("dynamic");
        write_dynamic_package(&package_root, "first_tool", "first", "First body").await;
        let manager = dynamic_skill_manager(&skills_root).await;
        let state = Arc::new(
            crate::api::AppState::new_with_agent(storage, Arc::new(TestAgent))
                .with_skill_manager(manager.clone())
                .with_runtime_config(
                    RuntimeConfig::workspace_write(workspace.clone(), workspace.clone())
                        .without_builtin_tools(),
                ),
        );
        let app = crate::api::router_with_dev_routes(state);

        let initial_tools = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/dev/tools")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(
            initial_tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| { tool["name"] == "first_tool" })
        );

        write_dynamic_package(&package_root, "second_tool", "second", "Second body").await;
        manager.reload().await.unwrap();

        let tools = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/dev/tools")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(
            tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| { tool["name"] == "second_tool" })
        );
        assert!(
            !tools["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| { tool["name"] == "first_tool" })
        );

        let discovery = read_json(
            app.clone()
                .oneshot(
                    Request::builder()
                        .uri("/dev/tool-discovery")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap(),
        )
        .await;
        assert!(
            discovery["tools"]
                .as_array()
                .unwrap()
                .iter()
                .any(|tool| { tool["name"] == "second_tool" })
        );

        let preview = read_json(
            app.oneshot(json_request(
                "/dev/instructions/preview",
                json!({ "content": "use $second" }),
            ))
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(preview["triggered_skills"], json!(["second"]));
        assert!(
            preview["developer"]
                .as_str()
                .unwrap()
                .contains("Second body")
        );
        assert!(
            !preview["developer"]
                .as_str()
                .unwrap()
                .contains("First body")
        );
        remove_test_dir(workspace).await;
    }

    async fn development_skills() -> PathBuf {
        let root = unique_test_dir("dev-tools");
        let skill_dir = root.join("echo");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            skill_dir.join("skill.json"),
            serde_json::json!({
                "name": "echo",
                "description": "Echo a text payload.",
                "version": "0.1.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [
                    {
                        "name": "echo",
                        "description": "Return the provided text.",
                        "input_schema": { "type": "object" }
                    }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            skill_dir.join("index.js"),
            "process.stdin.resume();\nprocess.stdin.on('data', (chunk) => process.stdout.write(chunk));\n",
        )
        .await
        .unwrap();
        root
    }

    async fn dynamic_skill_manager(root: &std::path::Path) -> SkillManager {
        SkillManager::new(SkillManagerConfig {
            sources: vec![Arc::new(DirectorySkillSource::new(
                SkillLayer::Builtin,
                root,
            ))],
            platform: PlatformId::Desktop,
            capabilities: CapabilitySet::desktop_runtime(),
            protected_packages: Vec::new(),
            allowed_overrides: Vec::new(),
            runtime_version: "0.1.0".parse().unwrap(),
        })
        .await
        .unwrap()
    }

    async fn write_dynamic_package(
        package_root: &std::path::Path,
        tool_name: &str,
        instruction_name: &str,
        instruction_body: &str,
    ) {
        tokio::fs::create_dir_all(package_root).await.unwrap();
        tokio::fs::write(
            package_root.join("general-agent.json"),
            json!({
                "schemaVersion": 1,
                "id": "com.example.dynamic",
                "version": "1.0.0",
                "displayName": "Dynamic",
                "kind": "native_runtime",
                "package": {
                    "includeInstructions": true,
                    "includeRuntime": true
                }
            })
            .to_string(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            package_root.join("skill.json"),
            json!({
                "name": "dynamic",
                "description": "Dynamic test skill.",
                "version": "1.0.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [{
                    "name": tool_name,
                    "description": "Dynamic tool.",
                    "input_schema": { "type": "object" }
                }]
            })
            .to_string(),
        )
        .await
        .unwrap();
        tokio::fs::write(package_root.join("index.js"), "process.stdin.resume();\n")
            .await
            .unwrap();
        tokio::fs::write(
            package_root.join("SKILL.md"),
            format!(
                "---\nname: {instruction_name}\ndescription: Dynamic instructions.\n---\n\n# Dynamic\n{instruction_body}"
            ),
        )
        .await
        .unwrap();
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "general-agent-dev-api-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.unwrap();
        }
    }

    async fn read_json(response: axum::response::Response) -> Value {
        let bytes = to_bytes(response.into_body(), 1024 * 1024).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn json_request(uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }
}
