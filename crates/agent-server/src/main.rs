use agent_runtime::{
    platform::{CapabilitySet, PlatformId},
    skill::SkillRegistry,
    skill_catalog::SkillCatalog,
    skill_manager::{SkillManager, SkillManagerConfig},
    skill_source::{DirectorySkillSource, SkillLayer},
    storage::Storage,
    tools::{CommandMode, RuntimeConfig},
};
use agent_server::api;
use model_gateway::{
    provider::{EndpointType, ProviderProfile},
    responses::GatewayHttpClient,
};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};

const DEFAULT_DATABASE_URL: &str = "sqlite://general-agent.db?mode=rwc";
const DEFAULT_SKILLS_ROOT: &str = "skills";
const DEFAULT_MODEL_BASE_URL: &str = "http://127.0.0.1:11434/v1";
const DEFAULT_MODEL_NAME: &str = "local-agent-model";

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let database_url =
        std::env::var("GENERAL_AGENT_DATABASE_URL").unwrap_or_else(|_| DEFAULT_DATABASE_URL.into());
    let storage = Storage::connect(&database_url).await?;
    let skills_root = skills_root_from_env();
    let skill_manager = load_skill_manager(&skills_root).await?;
    let model = GatewayHttpClient::new(model_profile_from_env());
    let runtime_config = runtime_config_from_env();
    let state = Arc::new(
        api::AppState::new_with_model_and_skill_manager(
            storage,
            model,
            skill_manager,
            runtime_config,
        )
        .with_skills_root(skills_root.clone()),
    );
    let app = if std::env::var("GENERAL_AGENT_DEV_API").as_deref() == Ok("1") {
        api::router_with_dev_routes(state)
    } else {
        api::router(state)
    };
    let addr = SocketAddr::from(([127, 0, 0, 1], 49321));
    let listener = tokio::net::TcpListener::bind(addr).await?;

    tracing::info!("agent server listening on http://{addr}");
    axum::serve(listener, app).await?;
    Ok(())
}

fn runtime_config_from_env() -> RuntimeConfig {
    let workspace_root = std::env::var("GENERAL_AGENT_WORKSPACE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let mut config = RuntimeConfig::workspace_write(workspace_root.clone(), workspace_root)
        .without_builtin_tools();
    if std::env::var("GENERAL_AGENT_COMMAND_MODE").as_deref() == Ok("allowed") {
        config = config.with_command_mode(CommandMode::Allowed);
    }
    config
}

fn skills_root_from_env() -> PathBuf {
    std::env::var("GENERAL_AGENT_SKILLS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SKILLS_ROOT))
}

async fn load_skill_manager(root: &Path) -> anyhow::Result<SkillManager> {
    if root.join("skill-bundle.json").is_file() {
        let registry = SkillRegistry::load_packaged(root).await?;
        let catalog = load_packaged_instruction_skills(root).await;
        return Ok(SkillManager::from_registry_and_catalog(registry, catalog));
    }

    SkillManager::new(SkillManagerConfig {
        sources: vec![Arc::new(DirectorySkillSource::new(
            SkillLayer::Builtin,
            root,
        ))],
        platform: PlatformId::Desktop,
        capabilities: CapabilitySet::desktop_runtime(),
        protected_packages: Vec::new(),
        allowed_overrides: Vec::new(),
        runtime_version: env!("CARGO_PKG_VERSION").parse()?,
    })
    .await
}

async fn load_packaged_instruction_skills(root: &Path) -> SkillCatalog {
    let result = SkillCatalog::load_packaged(root).await;

    result.unwrap_or_else(|error| {
        tracing::warn!(?error, "failed to load instruction skill catalog");
        SkillCatalog::empty()
    })
}

fn model_profile_from_env() -> ProviderProfile {
    ProviderProfile {
        id: "default".into(),
        name: "Default".into(),
        endpoint_type: model_endpoint_type_from_env(),
        base_url: std::env::var("GENERAL_AGENT_MODEL_BASE_URL")
            .unwrap_or_else(|_| DEFAULT_MODEL_BASE_URL.into()),
        model: std::env::var("GENERAL_AGENT_MODEL_NAME")
            .unwrap_or_else(|_| DEFAULT_MODEL_NAME.into()),
        api_key: std::env::var("GENERAL_AGENT_MODEL_API_KEY").ok(),
        headers: BTreeMap::new(),
    }
}

fn model_endpoint_type_from_env() -> EndpointType {
    match std::env::var("GENERAL_AGENT_MODEL_ENDPOINT_TYPE")
        .unwrap_or_else(|_| "chat_completions".into())
        .as_str()
    {
        "responses" => EndpointType::Responses,
        "completion" => EndpointType::Completion,
        _ => EndpointType::ChatCompletions,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_runtime::turn::{ModelClient, ModelEventStream};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use futures::stream;
    use model_gateway::responses::{GatewayEvent, GatewayRequest};
    use std::path::Path;
    use std::sync::Mutex;
    use tower::ServiceExt;

    struct CapturingModel {
        tool_names: Arc<Mutex<Vec<String>>>,
    }

    #[async_trait::async_trait]
    impl ModelClient for CapturingModel {
        async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream> {
            *self.tool_names.lock().unwrap() =
                request.tools.into_iter().map(|tool| tool.name).collect();
            Ok(Box::pin(stream::iter(vec![
                Ok(GatewayEvent::TextDelta {
                    text: "done".into(),
                }),
                Ok(GatewayEvent::Completed),
            ])))
        }
    }

    #[test]
    fn server_runtime_config_disables_builtin_tools_by_default() {
        assert!(!runtime_config_from_env().built_in_tools_enabled);
    }

    #[tokio::test]
    async fn production_state_and_runner_share_one_skill_manager() {
        let root = unique_test_dir("shared-manager");
        let package_root = root.join("runtime");
        write_runtime_package(&package_root, "first_tool").await;
        let manager = load_skill_manager(&root).await.unwrap();
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let session = storage.create_session("Shared manager").await.unwrap();
        let tool_names = Arc::new(Mutex::new(Vec::new()));
        let state = Arc::new(
            api::AppState::new_with_model_and_skill_manager(
                storage,
                CapturingModel {
                    tool_names: tool_names.clone(),
                },
                manager.clone(),
                RuntimeConfig::workspace_write(root.clone(), root.clone()).without_builtin_tools(),
            )
            .with_skills_root(root.clone()),
        );

        write_runtime_package(&package_root, "second_tool").await;
        manager.reload().await.unwrap();
        let response = api::router(state)
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/sessions/{}/messages", session.id))
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"content":"check tools"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let names = tool_names.lock().unwrap();
        assert!(names.iter().any(|name| name == "second_tool"));
        assert!(!names.iter().any(|name| name == "first_tool"));
        remove_test_dir(root).await;
    }

    #[tokio::test]
    async fn legacy_bundle_selection_uses_a_static_skill_manager() {
        let root = unique_test_dir("bundle-manager");
        let package_root = root.join("runtime");
        write_runtime_package(&package_root, "bundle_tool").await;
        tokio::fs::write(
            root.join("skill-bundle.json"),
            serde_json::json!({
                "skills": [{
                    "path": "runtime",
                    "includeInstructions": false
                }]
            })
            .to_string(),
        )
        .await
        .unwrap();

        let manager = load_skill_manager(&root).await.unwrap();

        assert_eq!(
            manager.current_snapshot().registry().tools()[0].name,
            "bundle_tool"
        );
        assert!(manager.reload().await.is_err());
        remove_test_dir(root).await;
    }

    async fn write_runtime_package(package_root: &Path, tool_name: &str) {
        tokio::fs::create_dir_all(package_root).await.unwrap();
        tokio::fs::write(
            package_root.join("general-agent.json"),
            serde_json::json!({
                "schemaVersion": 1,
                "id": "com.example.server-runtime",
                "version": "1.0.0",
                "displayName": "Server runtime",
                "kind": "native_runtime",
                "package": {
                    "includeInstructions": false,
                    "includeRuntime": true
                }
            })
            .to_string(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            package_root.join("skill.json"),
            serde_json::json!({
                "name": "server-runtime",
                "description": "Server runtime test skill.",
                "version": "1.0.0",
                "entry": {
                    "type": "command",
                    "command": "node",
                    "args": ["index.js"]
                },
                "tools": [{
                    "name": tool_name,
                    "description": "Test tool.",
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
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "general-agent-main-{name}-{}",
            uuid::Uuid::new_v4()
        ))
    }

    async fn remove_test_dir(path: PathBuf) {
        if path.exists() {
            tokio::fs::remove_dir_all(path).await.unwrap();
        }
    }
}
