use agent_runtime::{
    events::RuntimeEvent,
    session::Message,
    skill::SkillRegistry,
    skill_catalog::SkillCatalog,
    storage::Storage,
    tools::RuntimeConfig,
    turn::{AgentRunner, TurnRunner},
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
use tower_http::cors::CorsLayer;

#[derive(Clone)]
pub struct AppState {
    storage: Storage,
    agent: Arc<dyn AgentRunner>,
    skills: Option<SkillRegistry>,
    skills_root: Option<PathBuf>,
    skill_catalog: SkillCatalog,
    runtime_config: RuntimeConfig,
}

impl AppState {
    #[cfg(test)]
    pub fn new(storage: Storage) -> Self {
        Self::new_with_agent(storage, Arc::new(DeterministicAgent))
    }

    #[cfg(test)]
    pub fn new_with_agent(storage: Storage, agent: Arc<dyn AgentRunner>) -> Self {
        Self {
            storage,
            agent,
            skills: None,
            skills_root: None,
            skill_catalog: SkillCatalog::empty(),
            runtime_config: default_runtime_config(),
        }
    }

    pub fn new_with_agent_and_skills(
        storage: Storage,
        agent: Arc<dyn AgentRunner>,
        skills: SkillRegistry,
    ) -> Self {
        Self {
            storage,
            agent,
            skills: Some(skills),
            skills_root: None,
            skill_catalog: SkillCatalog::empty(),
            runtime_config: default_runtime_config(),
        }
    }

    pub fn with_runtime_config(mut self, runtime_config: RuntimeConfig) -> Self {
        self.runtime_config = runtime_config;
        self
    }

    pub fn with_skill_catalog(mut self, skill_catalog: SkillCatalog) -> Self {
        self.skill_catalog = skill_catalog;
        self
    }

    pub fn with_skills_root(mut self, skills_root: PathBuf) -> Self {
        self.skills_root = Some(skills_root);
        self
    }
}

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
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/model/test", post(test_model_connection))
        .route("/sessions", post(create_session))
        .route("/sessions/{session_id}/messages", post(post_message))
        .layer(desktop_cors_layer())
        .with_state(state)
}

pub fn router_with_dev_routes(state: Arc<AppState>) -> Router {
    router(state.clone()).merge(crate::dev_api::router(state).layer(desktop_cors_layer()))
}

impl AppState {
    pub(crate) fn skills(&self) -> Option<SkillRegistry> {
        self.skills.clone()
    }

    pub(crate) fn runtime_config(&self) -> RuntimeConfig {
        self.runtime_config.clone()
    }

    pub(crate) fn skill_catalog(&self) -> SkillCatalog {
        self.skill_catalog.clone()
    }

    pub(crate) fn skills_root(&self) -> Option<PathBuf> {
        self.skills_root.clone()
    }
}

pub(crate) fn desktop_cors_layer() -> CorsLayer {
    CorsLayer::new()
        .allow_origin([
            HeaderValue::from_static("http://127.0.0.1:5173"),
            HeaderValue::from_static("http://localhost:5173"),
        ])
        .allow_methods([Method::GET, Method::POST, Method::DELETE])
        .allow_headers([header::CONTENT_TYPE])
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
        let skills = state
            .skills
            .clone()
            .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("runtime skills unavailable")))?;
        let profile = provider_profile_from_request(model_settings)?;
        let runner = TurnRunner::new_with_catalog_and_config(
            GatewayHttpClient::new(profile),
            skills,
            state.skill_catalog.clone(),
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
mod tests {
    use super::*;
    use agent_runtime::{
        skill::SkillRegistry,
        storage::Storage,
        tools::RuntimeConfig,
        turn::{ModelClient, ModelEventStream, TurnRunner},
    };
    use async_trait::async_trait;
    use axum::body::Body;
    use axum::body::to_bytes;
    use axum::http::{HeaderMap, Request, StatusCode, header};
    use futures::stream;
    use model_gateway::responses::{GatewayEvent, GatewayRequest};
    use serde_json::{Value, json};
    use std::path::{Path, PathBuf};
    use std::sync::{
        Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    use tower::ServiceExt;

    struct SkillCallingModel {
        calls: AtomicUsize,
    }

    #[async_trait]
    impl ModelClient for SkillCallingModel {
        async fn stream(&self, request: GatewayRequest) -> anyhow::Result<ModelEventStream> {
            let call = self.calls.fetch_add(1, Ordering::SeqCst);
            assert!(request.tools.iter().any(|tool| tool.name == "echo"));
            let events = if call == 0 {
                vec![
                    Ok(GatewayEvent::ToolCall {
                        call_id: "call-1".into(),
                        name: "echo".into(),
                        arguments: json!({ "text": "hidden skill result" }),
                    }),
                    Ok(GatewayEvent::Completed),
                ]
            } else {
                vec![
                    Ok(GatewayEvent::TextDelta {
                        text: "The hidden capability returned hidden skill result.".into(),
                    }),
                    Ok(GatewayEvent::Completed),
                ]
            };

            Ok(Box::pin(stream::iter(events)))
        }
    }

    #[tokio::test]
    async fn health_returns_ok() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = router(Arc::new(AppState::new(storage)));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn create_session_and_post_message_returns_runtime_events() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = router(Arc::new(AppState::new(storage.clone())));

        let create_response = app
            .clone()
            .oneshot(json_request(
                "/sessions",
                json!({ "title": "MVP Verification" }),
            ))
            .await
            .unwrap();
        assert_eq!(create_response.status(), StatusCode::OK);
        let created = read_json(create_response).await;
        let session_id = created["id"].as_str().unwrap();
        assert_eq!(created["title"], "MVP Verification");

        let message_response = app
            .oneshot(json_request(
                &format!("/sessions/{session_id}/messages"),
                json!({ "content": "Run the renderer smoke test" }),
            ))
            .await
            .unwrap();

        assert_eq!(message_response.status(), StatusCode::OK);
        let message = read_json(message_response).await;
        assert_eq!(message["accepted"], true);
        assert_eq!(
            message["assistant_message"]["content"],
            "MVP agent received: Run the renderer smoke test"
        );
        assert_eq!(message["events"][0]["type"], "turn_started");
        assert_eq!(message["events"][1]["type"], "assistant_text_delta");
        assert_eq!(
            message["events"][2],
            json!({
                "type": "assistant_message_finished",
                "text": "MVP agent received: Run the renderer smoke test"
            })
        );
        assert_eq!(message["events"][3]["type"], "turn_finished");

        let stored_messages = storage.list_messages(session_id).await.unwrap();
        assert_eq!(stored_messages.len(), 2);
        assert_eq!(stored_messages[0].role, "user");
        assert_eq!(stored_messages[0].content, "Run the renderer smoke test");
        assert_eq!(stored_messages[1].role, "assistant");
        assert_eq!(
            stored_messages[1].content,
            "MVP agent received: Run the renderer smoke test"
        );
    }

    #[tokio::test]
    async fn post_message_runs_packaged_skills_through_the_agent_turn_loop() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let skills_root = packaged_echo_skill().await;
        let skills = SkillRegistry::load_packaged(&skills_root).await.unwrap();
        let runner = TurnRunner::new(
            SkillCallingModel {
                calls: AtomicUsize::new(0),
            },
            skills,
        );
        let app = router(Arc::new(AppState::new_with_agent(
            storage.clone(),
            Arc::new(runner),
        )));

        let create_response = app
            .clone()
            .oneshot(json_request(
                "/sessions",
                json!({ "title": "Hidden skill" }),
            ))
            .await
            .unwrap();
        let created = read_json(create_response).await;
        let session_id = created["id"].as_str().unwrap();

        let message_response = app
            .oneshot(json_request(
                &format!("/sessions/{session_id}/messages"),
                json!({ "content": "Use the hidden echo capability" }),
            ))
            .await
            .unwrap();

        assert_eq!(message_response.status(), StatusCode::OK);
        let message = read_json(message_response).await;
        assert_eq!(
            message["assistant_message"]["content"],
            "The hidden capability returned hidden skill result."
        );
        assert_eq!(message["events"][0]["type"], "turn_started");
        assert!(
            message["events"]
                .as_array()
                .unwrap()
                .iter()
                .any(|event| event["type"] == "tool_call_finished")
        );

        let stored_messages = storage.list_messages(session_id).await.unwrap();
        assert_eq!(
            stored_messages[1].content,
            "The hidden capability returned hidden skill result."
        );

        remove_test_dir(skills_root).await;
    }

    #[tokio::test]
    async fn post_message_rejects_missing_session() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = router(Arc::new(AppState::new(storage)));

        let response = app
            .oneshot(json_request(
                "/sessions/missing-session/messages",
                json!({ "content": "hello" }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = read_json(response).await;
        assert_eq!(body["error"], "session not found");
    }

    #[tokio::test]
    async fn test_model_connection_uses_supplied_chat_completions_profile() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let provider_capture = Arc::new(ProviderCapture::default());
        let provider_base_url = spawn_provider_server(provider_capture.clone()).await;
        let app = router(Arc::new(AppState::new(storage)));

        let response = app
            .oneshot(json_request(
                "/model/test",
                json!({
                    "apiKey": "local-secret",
                    "baseUrl": provider_base_url,
                    "endpointType": "chat_completions",
                    "modelName": "qwen2.5"
                }),
            ))
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = read_json(response).await;
        assert_eq!(body["ok"], true);
        assert_eq!(body["message"], "Connection succeeded");

        let captured = provider_capture
            .request
            .lock()
            .unwrap()
            .clone()
            .expect("provider should receive test request");
        assert_eq!(captured.authorization, Some("Bearer local-secret".into()));
        assert_eq!(captured.body["model"], "qwen2.5");
        assert_eq!(captured.body["stream"], false);
        assert_eq!(captured.body["messages"][0]["role"], "user");
    }

    #[tokio::test]
    async fn post_message_uses_supplied_model_settings_for_agent_turn() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let provider_capture = Arc::new(ProviderCapture::default());
        let provider_base_url = spawn_provider_server(provider_capture.clone()).await;
        let workspace = unique_test_dir("server-configured-provider-workspace");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        tokio::fs::write(
            workspace.join("AGENTS.md"),
            "Project instruction from configured workspace",
        )
        .await
        .unwrap();
        let runtime_config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
        let skills = development_skills().await;
        let app = router(Arc::new(
            AppState::new_with_agent_and_skills(
                storage.clone(),
                Arc::new(DeterministicAgent),
                skills,
            )
            .with_runtime_config(runtime_config),
        ));

        let create_response = app
            .clone()
            .oneshot(json_request(
                "/sessions",
                json!({ "title": "Configured provider" }),
            ))
            .await
            .unwrap();
        let created = read_json(create_response).await;
        let session_id = created["id"].as_str().unwrap();

        let message_response = app
            .oneshot(json_request(
                &format!("/sessions/{session_id}/messages"),
                json!({
                    "content": "Use the configured provider",
                    "modelSettings": {
                        "apiKey": "local-secret",
                        "baseUrl": provider_base_url,
                        "endpointType": "chat_completions",
                        "modelName": "qwen2.5"
                    }
                }),
            ))
            .await
            .unwrap();

        assert_eq!(message_response.status(), StatusCode::OK);
        let message = read_json(message_response).await;
        assert_eq!(message["assistant_message"]["content"], "ok");

        let captured = provider_capture
            .request
            .lock()
            .unwrap()
            .clone()
            .expect("provider should receive chat request");
        assert_eq!(captured.authorization, Some("Bearer local-secret".into()));
        assert_eq!(captured.body["model"], "qwen2.5");
        let messages = captured.body["messages"].as_array().unwrap();
        assert!(messages.iter().any(|message| message["role"] == "system"));
        let project_instruction = messages
            .iter()
            .find(|message| {
                message["role"] == "system"
                    && message["content"]
                        .as_str()
                        .is_some_and(|content| content.contains("Project instruction"))
            })
            .and_then(|message| message["content"].as_str())
            .expect("project instruction system message should be present");
        assert!(project_instruction.contains("Project instruction from configured workspace"));
        assert!(messages.iter().any(|message| {
            message["role"] == "user" && message["content"] == "Use the configured provider"
        }));

        remove_test_dir(workspace).await;
    }

    #[tokio::test]
    async fn post_message_rejects_completion_endpoint_for_tool_using_turns() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let skills = development_skills().await;
        let app = router(Arc::new(AppState::new_with_agent_and_skills(
            storage,
            Arc::new(DeterministicAgent),
            skills,
        )));

        let create_response = app
            .clone()
            .oneshot(json_request(
                "/sessions",
                json!({ "title": "Completion endpoint" }),
            ))
            .await
            .unwrap();
        let created = read_json(create_response).await;
        let session_id = created["id"].as_str().unwrap();

        let message_response = app
            .oneshot(json_request(
                &format!("/sessions/{session_id}/messages"),
                json!({
                    "content": "Use completion endpoint",
                    "modelSettings": {
                        "baseUrl": "http://127.0.0.1:9/v1",
                        "endpointType": "completion",
                        "modelName": "legacy"
                    }
                }),
            ))
            .await
            .unwrap();

        assert_eq!(message_response.status(), StatusCode::BAD_REQUEST);
        let body = read_json(message_response).await;
        assert_eq!(
            body["error"],
            "model endpoint does not support runtime tools"
        );
    }

    #[tokio::test]
    async fn app_state_accepts_runtime_config_for_model_settings_turns() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let workspace = unique_test_dir("server-runtime-config");
        tokio::fs::create_dir_all(&workspace).await.unwrap();
        let runtime_config = RuntimeConfig::workspace_write(workspace.clone(), workspace.clone());
        let skills = development_skills().await;

        let state =
            AppState::new_with_agent_and_skills(storage, Arc::new(DeterministicAgent), skills)
                .with_runtime_config(runtime_config);

        assert_eq!(state.runtime_config.workspace_root, workspace);
        assert_eq!(
            state.runtime_config.cwd,
            state.runtime_config.workspace_root
        );

        remove_test_dir(state.runtime_config.workspace_root).await;
    }

    #[test]
    fn internal_api_errors_return_500() {
        let response = ApiError::Internal(anyhow::anyhow!("storage unavailable")).into_response();

        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    #[test]
    fn upstream_model_errors_return_bad_gateway() {
        let response = agent_turn_error(anyhow::anyhow!(
            "upstream model request failed: 400 Bad Request"
        ))
        .into_response();

        assert_eq!(response.status(), StatusCode::BAD_GATEWAY);
    }

    #[tokio::test]
    async fn supports_vite_desktop_cors_preflight() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = router(Arc::new(AppState::new(storage)));

        for origin in ["http://127.0.0.1:5173", "http://localhost:5173"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("OPTIONS")
                        .uri("/sessions/session-1/messages")
                        .header(header::ORIGIN, origin)
                        .header(header::ACCESS_CONTROL_REQUEST_METHOD, "POST")
                        .header(header::ACCESS_CONTROL_REQUEST_HEADERS, "content-type")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::OK);
            assert_eq!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_ORIGIN],
                origin
            );
            assert!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_METHODS]
                    .to_str()
                    .unwrap()
                    .contains("POST")
            );
            assert!(
                response.headers()[header::ACCESS_CONTROL_ALLOW_HEADERS]
                    .to_str()
                    .unwrap()
                    .contains("content-type")
            );
        }
    }

    #[tokio::test]
    async fn production_router_does_not_expose_skill_inventory() {
        let storage = Storage::connect("sqlite::memory:").await.unwrap();
        let app = router(Arc::new(AppState::new(storage)));

        for uri in ["/skills", "/dev/skills"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .method("GET")
                        .uri(uri)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();

            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
    }

    fn json_request(uri: &str, body: Value) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri(uri)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    }

    async fn read_json(response: axum::response::Response) -> Value {
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[derive(Clone, Debug)]
    struct CapturedProviderRequest {
        authorization: Option<String>,
        body: Value,
    }

    #[derive(Default)]
    struct ProviderCapture {
        request: Mutex<Option<CapturedProviderRequest>>,
    }

    async fn spawn_provider_server(capture: Arc<ProviderCapture>) -> String {
        let app = Router::new()
            .route("/v1/chat/completions", post(capture_provider_request))
            .with_state(capture);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        format!("http://{addr}/v1")
    }

    async fn capture_provider_request(
        State(capture): State<Arc<ProviderCapture>>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let authorization = headers
            .get(header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        *capture.request.lock().unwrap() = Some(CapturedProviderRequest {
            authorization,
            body,
        });

        Json(json!({
            "choices": [
                {
                    "message": {
                        "content": "ok"
                    }
                }
            ]
        }))
    }

    async fn development_skills() -> SkillRegistry {
        let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(Path::parent)
            .unwrap()
            .join("skills");
        SkillRegistry::load_development(skills_root).await.unwrap()
    }

    async fn packaged_echo_skill() -> PathBuf {
        let root = unique_test_dir("server-packaged-echo");
        let skill_dir = root.join("echo");
        tokio::fs::create_dir_all(&skill_dir).await.unwrap();
        tokio::fs::write(
            root.join("skill-bundle.json"),
            json!({
                "skills": [
                    { "path": "echo" }
                ]
            })
            .to_string(),
        )
        .await
        .unwrap();
        tokio::fs::write(
            skill_dir.join("skill.json"),
            json!({
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
                        "input_schema": {
                            "type": "object",
                            "properties": {
                                "text": { "type": "string" }
                            },
                            "required": ["text"]
                        }
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
        std::env::temp_dir().join(format!("generalagent-{name}-{}", uuid::Uuid::new_v4()))
    }

    async fn remove_test_dir(path: impl AsRef<Path>) {
        if path.as_ref().exists() {
            tokio::fs::remove_dir_all(path).await.unwrap();
        }
    }
}
