mod api;
mod dev_api;
mod dev_skills;

use agent_runtime::{
    skill::SkillRegistry,
    skill_catalog::SkillCatalog,
    storage::Storage,
    tools::{CommandMode, RuntimeConfig},
    turn::TurnRunner,
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
    let skills_root = skills_root_from_env();
    let skills = load_runtime_skills(&skills_root).await?;
    let skill_catalog = load_instruction_skills(&skills_root).await;
    let model = GatewayHttpClient::new(model_profile_from_env());
    let runtime_config = runtime_config_from_env();
    let runner = TurnRunner::new_with_catalog_and_config(
        model,
        skills.clone(),
        skill_catalog.clone(),
        runtime_config.clone(),
    );
    let state = Arc::new(
        api::AppState::new_with_agent_and_skills(storage, Arc::new(runner), skills)
            .with_runtime_config(runtime_config)
            .with_skill_catalog(skill_catalog),
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
    let mut config = RuntimeConfig::workspace_write(workspace_root.clone(), workspace_root);
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

async fn load_runtime_skills(root: &PathBuf) -> anyhow::Result<SkillRegistry> {
    if root.join("skill-bundle.json").is_file() {
        SkillRegistry::load_packaged(root).await
    } else {
        SkillRegistry::load_development(root).await
    }
}

async fn load_instruction_skills(root: &PathBuf) -> SkillCatalog {
    let result = if root.join("skill-bundle.json").is_file() {
        SkillCatalog::load_packaged(root).await
    } else {
        SkillCatalog::load_development(root).await
    };

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
