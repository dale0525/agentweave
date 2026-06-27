use agent_runtime::{
    instructions::{InstructionConfig, InstructionContext},
    tools::{ToolRegistry, schema::ToolDiagnostic},
};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::api::AppState;

#[derive(Debug, Serialize)]
struct DevToolsResponse {
    tools: Vec<ToolDiagnostic>,
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
        .route("/dev/instructions/preview", post(preview_instructions))
        .with_state(state)
}

async fn list_tools(
    State(state): State<Arc<AppState>>,
) -> Result<Json<DevToolsResponse>, StatusCode> {
    let skills = state.skills().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    let registry = ToolRegistry::new(skills, &state.runtime_config());

    Ok(Json(DevToolsResponse {
        tools: registry.diagnostics(),
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

#[cfg(test)]
mod tests {
    use agent_runtime::{
        events::RuntimeEvent, skill::SkillRegistry, skill_catalog::SkillCatalog, storage::Storage,
        tools::RuntimeConfig, turn::AgentRunner,
    };
    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
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
        assert!(tools.iter().any(|tool| tool["name"] == "create_directory"));
        assert!(tools.iter().all(|tool| tool.get("schema").is_some()));
        remove_test_dir(skills_root).await;
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
