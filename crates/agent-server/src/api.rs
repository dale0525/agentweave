use crate::owner_api::OwnerApiConfig;
use agent_runtime::{
    events::RuntimeEvent,
    session::Message,
    skill::SkillRegistry,
    skill_catalog::SkillCatalog,
    skill_manager::SkillManager,
    storage::Storage,
    tools::RuntimeConfig,
    turn::{AgentRunner, ModelClient, TurnRunner},
};
#[cfg(test)]
use async_trait::async_trait;
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use model_gateway::{
    provider::{EndpointType, ProviderProfile},
    responses::{GatewayHttpClient, GatewayRequest},
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct AppState {
    storage: Storage,
    agent: Arc<dyn AgentRunner>,
    skill_manager: SkillManager,
    skills_root: Option<PathBuf>,
    runtime_config: RuntimeConfig,
    dev_skill_mutations: Arc<Mutex<()>>,
    owner_management: Option<OwnerApiConfig>,
}

impl AppState {
    pub fn new_with_model_and_skill_manager<C>(
        storage: Storage,
        model: C,
        skill_manager: SkillManager,
        runtime_config: RuntimeConfig,
    ) -> Self
    where
        C: ModelClient + 'static,
    {
        let runner = TurnRunner::new_with_manager_and_config(
            model,
            skill_manager.clone(),
            runtime_config.clone(),
        );
        Self {
            storage,
            agent: Arc::new(runner),
            skill_manager,
            skills_root: None,
            runtime_config,
            dev_skill_mutations: Arc::new(Mutex::new(())),
            owner_management: None,
        }
    }

    pub fn new_with_model_skill_manager_and_owner<C>(
        storage: Storage,
        model: C,
        skill_manager: SkillManager,
        runtime_config: RuntimeConfig,
        owner_management: OwnerApiConfig,
    ) -> Self
    where
        C: ModelClient + 'static,
    {
        let runner = TurnRunner::new_with_manager_and_config(
            model,
            skill_manager.clone(),
            runtime_config.clone(),
        )
        .with_skill_management(owner_management.tool_context());
        Self {
            storage,
            agent: Arc::new(runner),
            skill_manager,
            skills_root: None,
            runtime_config,
            dev_skill_mutations: Arc::new(Mutex::new(())),
            owner_management: Some(owner_management),
        }
    }

    #[cfg(test)]
    pub fn new(storage: Storage) -> Self {
        Self::new_with_agent(storage, Arc::new(DeterministicAgent))
    }

    #[cfg(test)]
    pub fn new_with_agent(storage: Storage, agent: Arc<dyn AgentRunner>) -> Self {
        Self::new_with_agent_and_skill_manager(
            storage,
            agent,
            SkillManager::from_registry_and_catalog(SkillRegistry::empty(), SkillCatalog::empty()),
        )
    }

    #[cfg(test)]
    pub fn new_with_agent_and_skills(
        storage: Storage,
        agent: Arc<dyn AgentRunner>,
        skills: SkillRegistry,
    ) -> Self {
        Self::new_with_agent_and_skill_manager(
            storage,
            agent,
            SkillManager::from_registry_and_catalog(skills, SkillCatalog::empty()),
        )
    }

    #[cfg(test)]
    pub fn new_with_agent_and_skill_manager(
        storage: Storage,
        agent: Arc<dyn AgentRunner>,
        skill_manager: SkillManager,
    ) -> Self {
        Self {
            storage,
            agent,
            skill_manager,
            skills_root: None,
            runtime_config: default_runtime_config(),
            dev_skill_mutations: Arc::new(Mutex::new(())),
            owner_management: None,
        }
    }

    #[cfg(test)]
    pub fn with_runtime_config(mut self, runtime_config: RuntimeConfig) -> Self {
        self.runtime_config = runtime_config;
        self
    }

    #[cfg(test)]
    pub fn with_skill_catalog(mut self, skill_catalog: SkillCatalog) -> Self {
        let registry = self.skill_manager.current_snapshot().registry().clone();
        self.skill_manager = SkillManager::from_registry_and_catalog(registry, skill_catalog);
        self
    }

    #[cfg(test)]
    pub fn with_skill_manager(mut self, skill_manager: SkillManager) -> Self {
        self.skill_manager = skill_manager;
        self
    }

    pub fn with_skills_root(mut self, skills_root: PathBuf) -> Self {
        self.skills_root = Some(skills_root);
        self
    }
}

#[cfg(test)]
fn default_runtime_config() -> RuntimeConfig {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    RuntimeConfig::workspace_write(cwd.clone(), cwd).without_builtin_tools()
}

#[cfg(test)]
struct DeterministicAgent;

#[cfg(test)]
#[async_trait]
impl AgentRunner for DeterministicAgent {
    async fn run(&self, user_text: &str) -> anyhow::Result<Vec<RuntimeEvent>> {
        let turn_id = uuid::Uuid::new_v4().to_string();
        let assistant_text = deterministic_assistant_reply(user_text);

        Ok(vec![
            RuntimeEvent::TurnStarted {
                turn_id: turn_id.clone(),
            },
            RuntimeEvent::AssistantTextDelta {
                text: assistant_text.clone(),
            },
            RuntimeEvent::AssistantMessageFinished {
                text: assistant_text,
            },
            RuntimeEvent::TurnFinished { turn_id },
        ])
    }
}

#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub title: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CreateSessionResponse {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserMessageRequest {
    pub content: String,
    #[serde(default)]
    pub model_settings: Option<ModelConnectionTestRequest>,
}

#[derive(Debug, Serialize)]
pub struct UserMessageResponse {
    pub accepted: bool,
    pub user_message: Message,
    pub assistant_message: Message,
    pub events: Vec<RuntimeEvent>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelConnectionTestRequest {
    #[serde(default)]
    pub api_key: Option<String>,
    pub base_url: String,
    pub endpoint_type: EndpointType,
    pub model_name: String,
}

#[derive(Debug, Serialize)]
pub struct ModelConnectionTestResponse {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug)]
enum ApiError {
    BadRequest(&'static str),
    ConnectionFailed(anyhow::Error),
    NotFound(&'static str),
    Internal(anyhow::Error),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, error) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message.to_string()),
            Self::ConnectionFailed(error) => (
                StatusCode::BAD_GATEWAY,
                format!("connection failed: {error}"),
            ),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message.to_string()),
            Self::Internal(error) => {
                tracing::error!(?error, "agent-server request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "internal server error".to_string(),
                )
            }
        };

        (status, Json(ErrorResponse { error })).into_response()
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    let mut router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/model/test", post(test_model_connection))
        .route("/sessions", post(create_session))
        .route("/sessions/{session_id}/messages", post(post_message));
    if let Some(owner_routes) = crate::owner_api::router(&state) {
        router = router.merge(owner_routes);
    }
    router.layer(desktop_cors_layer()).with_state(state)
}

pub fn router_with_dev_routes(state: Arc<AppState>) -> Router {
    router(state.clone()).merge(crate::dev_api::router(state).layer(desktop_cors_layer()))
}

impl AppState {
    pub(crate) fn skills(&self) -> SkillRegistry {
        self.skill_manager.current_snapshot().registry().clone()
    }

    pub(crate) fn runtime_config(&self) -> RuntimeConfig {
        self.runtime_config.clone()
    }

    pub(crate) fn skill_catalog(&self) -> SkillCatalog {
        self.skill_manager.current_snapshot().catalog().clone()
    }

    pub(crate) fn skill_manager(&self) -> SkillManager {
        self.skill_manager.clone()
    }

    pub(crate) fn skills_root(&self) -> Option<PathBuf> {
        self.skills_root.clone()
    }

    pub(crate) fn dev_skill_mutations(&self) -> &Mutex<()> {
        &self.dev_skill_mutations
    }

    pub(crate) fn owner_management(&self) -> Option<&OwnerApiConfig> {
        self.owner_management.as_ref()
    }
}

pub(crate) fn desktop_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("http://127.0.0.1:5173"),
            HeaderValue::from_static("http://localhost:5173"),
        ])
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION])
}

async fn test_model_connection(
    Json(request): Json<ModelConnectionTestRequest>,
) -> Result<Json<ModelConnectionTestResponse>, ApiError> {
    let profile = provider_profile_from_request(request)?;
    let client = GatewayHttpClient::new(profile);

    let _events = client
        .stream(test_connection_gateway_request())
        .await
        .map_err(ApiError::ConnectionFailed)?;

    Ok(Json(ModelConnectionTestResponse {
        ok: true,
        message: "Connection succeeded".into(),
    }))
}

async fn create_session(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CreateSessionRequest>,
) -> Result<Json<CreateSessionResponse>, ApiError> {
    let title = request.title.unwrap_or_else(|| "New Session".to_string());
    let session = state
        .storage
        .create_session(&title)
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(CreateSessionResponse {
        id: session.id,
        title: session.title,
    }))
}

async fn post_message(
    Path(session_id): Path<String>,
    State(state): State<Arc<AppState>>,
    Json(request): Json<UserMessageRequest>,
) -> Result<Json<UserMessageResponse>, ApiError> {
    let session_exists = state
        .storage
        .session_exists(&session_id)
        .await
        .map_err(ApiError::Internal)?;
    if !session_exists {
        return Err(ApiError::NotFound("session not found"));
    }

    let events = run_agent_turn(&state, &request).await?;
    let assistant_text = assistant_text_from_events(&events)
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("agent turn did not finish")))?;
    let (user_message, assistant_message) = state
        .storage
        .append_turn(&session_id, &request.content, &assistant_text)
        .await
        .map_err(ApiError::Internal)?;

    Ok(Json(UserMessageResponse {
        accepted: true,
        user_message,
        assistant_message,
        events,
    }))
}

async fn run_agent_turn(
    state: &AppState,
    request: &UserMessageRequest,
) -> Result<Vec<RuntimeEvent>, ApiError> {
    if let Some(model_settings) = request.model_settings.clone() {
        let profile = provider_profile_from_request(model_settings)?;
        let runner = TurnRunner::new_with_manager_and_config(
            GatewayHttpClient::new(profile),
            state.skill_manager(),
            state.runtime_config.clone(),
        );

        return runner.run(&request.content).await.map_err(agent_turn_error);
    }

    state
        .agent
        .run(&request.content)
        .await
        .map_err(agent_turn_error)
}

fn agent_turn_error(error: anyhow::Error) -> ApiError {
    let message = error.to_string();
    if message.contains("model_endpoint_does_not_support_tools") {
        ApiError::BadRequest("model endpoint does not support runtime tools")
    } else if message.contains("upstream model request failed") {
        ApiError::ConnectionFailed(error)
    } else {
        ApiError::Internal(error)
    }
}

fn assistant_text_from_events(events: &[RuntimeEvent]) -> Option<String> {
    events.iter().find_map(|event| {
        if let RuntimeEvent::AssistantMessageFinished { text } = event {
            Some(text.clone())
        } else {
            None
        }
    })
}

fn provider_profile_from_request(
    request: ModelConnectionTestRequest,
) -> Result<ProviderProfile, ApiError> {
    let base_url = request.base_url.trim();
    if base_url.is_empty() {
        return Err(ApiError::BadRequest("base URL is required"));
    }

    let model = request.model_name.trim();
    if model.is_empty() {
        return Err(ApiError::BadRequest("model name is required"));
    }

    Ok(ProviderProfile {
        id: "settings-test".into(),
        name: "Settings Test".into(),
        endpoint_type: request.endpoint_type,
        base_url: base_url.to_string(),
        model: model.to_string(),
        api_key: request.api_key.and_then(|api_key| {
            let trimmed = api_key.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }),
        headers: BTreeMap::new(),
    })
}

fn test_connection_gateway_request() -> GatewayRequest {
    GatewayRequest {
        input: vec![serde_json::json!({
            "role": "user",
            "content": "Reply with ok to confirm this connection."
        })],
        tools: Vec::new(),
    }
}

#[cfg(test)]
fn deterministic_assistant_reply(content: &str) -> String {
    format!("MVP agent received: {content}")
}

#[cfg(test)]
#[path = "api_tests.rs"]
mod tests;
