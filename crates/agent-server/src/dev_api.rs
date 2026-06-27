use agent_runtime::tools::{ToolRegistry, schema::ToolDiagnostic};
use axum::{Json, Router, extract::State, http::StatusCode, routing::get};
use serde::Serialize;
use std::sync::Arc;

use crate::api::AppState;

#[derive(Debug, Serialize)]
struct DevToolsResponse {
    tools: Vec<ToolDiagnostic>,
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/dev/tools", get(list_tools))
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

#[cfg(test)]
mod tests {
    use agent_runtime::{
        events::RuntimeEvent, skill::SkillRegistry, storage::Storage, turn::AgentRunner,
    };
    use async_trait::async_trait;
    use axum::{
        body::{Body, to_bytes},
        http::{Request, StatusCode},
    };
    use serde_json::Value;
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
}
