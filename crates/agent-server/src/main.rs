mod api;

use agent_runtime::{
    skill::SkillRegistry, storage::Storage, tools::RuntimeConfig, turn::TurnRunner,
};
use model_gateway::{
    provider::{EndpointType, ProviderProfile},
    responses::GatewayHttpClient,
};
use std::{collections::BTreeMap, net::SocketAddr, path::PathBuf, sync::Arc};

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
    let skills = load_runtime_skills().await?;
    let model = GatewayHttpClient::new(model_profile_from_env());
    let runtime_config = runtime_config_from_env();
    let runner = TurnRunner::new_with_config(model, skills.clone(), runtime_config.clone());
    let app = api::router(Arc::new(
        api::AppState::new_with_agent_and_skills(storage, Arc::new(runner), skills)
            .with_runtime_config(runtime_config),
    ));
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
    RuntimeConfig::workspace_write(workspace_root.clone(), workspace_root)
}

async fn load_runtime_skills() -> anyhow::Result<SkillRegistry> {
    let root = std::env::var("GENERAL_AGENT_SKILLS_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(DEFAULT_SKILLS_ROOT));

    if root.join("skill-bundle.json").is_file() {
        SkillRegistry::load_packaged(root).await
    } else {
        SkillRegistry::load_development(root).await
    }
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
